// based on https://github.com/huggingface/candle/blob/main/candle-transformers/src/models/llama.rs

use crate::{DType, Device, IndexOp, Tensor, D};
use candle_core::Result;
use candle_nn::{linear_no_bias, Embedding, Linear, Module, RmsNorm, VarBuilder};
use serde::Deserialize;

use crate::{
    config::ModelConfig, get_trace, kernels, seq::BatchInfo, to_offsets, util::check_all_close,
};

const DOUBLE_CHECK: bool = false;

#[derive(Deserialize)]
pub struct LlamaConfig {
    pub hidden_size: usize,
    pub intermediate_size: usize,
    pub vocab_size: usize,
    pub num_hidden_layers: usize,
    pub num_attention_heads: usize,
    pub num_key_value_heads: Option<usize>,
    pub rms_norm_eps: f64,
    pub max_position_embeddings: usize, // TODO - is this max seq len?
    #[serde(default = "default_rope")]
    pub rope_theta: f32,
}

fn default_rope() -> f32 {
    10_000.0
}

impl LlamaConfig {
    pub fn into_config(self) -> ModelConfig {
        ModelConfig {
            hidden_size: self.hidden_size,
            intermediate_size: self.intermediate_size,
            vocab_size: self.vocab_size,
            num_hidden_layers: self.num_hidden_layers,
            num_attention_heads: self.num_attention_heads,
            num_key_value_heads: self.num_key_value_heads.unwrap_or(self.num_attention_heads),
            rms_norm_eps: Some(self.rms_norm_eps),
            rope_theta: Some(self.rope_theta),
            max_sequence_length: self.max_position_embeddings,
            dtype_str: "bf16".to_string(),
        }
    }
}

impl ModelConfig {
    pub fn config_7b_v2() -> Self {
        Self {
            num_attention_heads: 32,
            hidden_size: 4096,
            num_hidden_layers: 32,
            num_key_value_heads: 32,
            max_sequence_length: 4096, // ???
            dtype_str: "bf16".to_string(),
            intermediate_size: 11008,
            vocab_size: 32000,
            rms_norm_eps: Some(1e-5),
            rope_theta: Some(10_000.0),
        }
    }
}

#[derive(Clone)]
struct Cache {
    cos_sin: Tensor,
}

impl Cache {
    pub fn new(dtype: DType, config: &ModelConfig, device: &Device) -> Result<Self> {
        // precompute freqs_cis
        let rotary_dim = config.hidden_size / config.num_attention_heads;
        let theta: Vec<_> = (0..rotary_dim)
            .step_by(2)
            .map(|i| {
                1f32 / config
                    .rope_theta
                    .unwrap()
                    .powf(i as f32 / rotary_dim as f32)
            })
            .collect();
        let theta = Tensor::new(theta.as_slice(), device)?;
        let len = config.max_sequence_length;
        let idx_theta = Tensor::arange(0u32, len as u32, device)?
            .to_dtype(DType::F32)?
            .reshape((len, 1))?
            .matmul(&theta.reshape((1, theta.elem_count()))?)?;
        let cos = idx_theta.cos()?.to_dtype(dtype)?;
        let sin = idx_theta.sin()?.to_dtype(dtype)?;
        let cos_sin = Tensor::cat(&[&cos, &sin], D::Minus1)?.contiguous()?;
        Ok(Self { cos_sin })
    }
}

fn embedding(cfg: &ModelConfig, vb: VarBuilder) -> Result<Embedding> {
    let embeddings = vb.get((cfg.vocab_size, cfg.hidden_size), "weight")?;
    Ok(Embedding::new(embeddings, cfg.hidden_size))
}

struct CausalSelfAttention {
    q_proj: Linear,
    k_proj: Linear,
    v_proj: Linear,
    o_proj: Linear,
    num_attention_heads: usize,
    num_key_value_heads: usize,
    head_dim: usize,
    cache: Cache,
}

pub fn naive_attn(
    q: &Tensor,
    k: &Tensor,
    v: &Tensor,
    seqlens_q: &Tensor,
    seqlens_k: &Tensor,
    max_seqlen_q: usize,
    max_seqlen_k: usize,
    softmax_scale: f32,
    causal: bool,
) -> Result<Tensor> {
    let seqlens_q = seqlens_q.to_vec1::<u32>()?;
    let seqlens_k = seqlens_k.to_vec1::<u32>()?;
    assert!(seqlens_q.len() == seqlens_k.len());
    let batch_size = seqlens_k.len() - 1;

    let softmax_scale = softmax_scale as f64;

    // flash-attn expects (seq_len, nheads, head_dim)

    let mut attns = Vec::with_capacity(batch_size);

    for i in 0..batch_size {
        let ptr_q = seqlens_q[i] as usize;
        let ptr_k = seqlens_k[i] as usize;
        let len_q = seqlens_q[i + 1] as usize - ptr_q;
        let len_k = seqlens_k[i + 1] as usize - ptr_k;

        assert!(len_q <= max_seqlen_q);
        assert!(len_k <= max_seqlen_k);

        let q = q.i((ptr_q..ptr_q + len_q, .., ..))?.transpose(0, 1)?;
        let k = k.i((ptr_k..ptr_k + len_k, .., ..))?.transpose(0, 1)?;
        let v = v.i((ptr_k..ptr_k + len_k, .., ..))?.transpose(0, 1)?;

        let mut attn = q.contiguous()?.matmul(&k.t()?.contiguous()?)?;
        attn = (attn * softmax_scale)?;
        if causal {
            let mask: Vec<_> = (0..len_q)
                .flat_map(|i| {
                    (0..len_k).map(move |j| {
                        if i + (len_k - len_q) >= j {
                            0f32
                        } else {
                            f32::NEG_INFINITY
                        }
                    })
                })
                .collect();
            let mask =
                Tensor::from_slice(&mask, (len_q, len_k), q.device())?.to_dtype(attn.dtype())?;
            // println!("mask: {mask}");
            attn = attn.broadcast_add(&mask)?;
        }

        attn = candle_nn::ops::softmax(&attn, D::Minus1)?;
        // Convert to contiguous as matmul doesn't support strided vs for now.
        attn = attn.matmul(&v.contiguous()?)?;
        attn = attn.transpose(0, 1)?;

        attns.push(attn);
    }

    let attn = Tensor::cat(&attns, 0)?;
    Ok(attn)
}

pub fn flash_attn_by_piece(
    q: &Tensor,
    k: &Tensor,
    v: &Tensor,
    seqlens_q: &Tensor,
    seqlens_k: &Tensor,
    max_seqlen_q: usize,
    max_seqlen_k: usize,
    softmax_scale: f32,
    causal: bool,
) -> Result<Tensor> {
    let seqlens_q = seqlens_q.to_vec1::<u32>()?;
    let seqlens_k = seqlens_k.to_vec1::<u32>()?;
    assert!(seqlens_q.len() == seqlens_k.len());
    let batch_size = seqlens_k.len() - 1;

    // flash-attn expects (seq_len, nheads, head_dim)

    let mut attns = Vec::with_capacity(batch_size);

    for i in 0..batch_size {
        let ptr_q = seqlens_q[i] as usize;
        let ptr_k = seqlens_k[i] as usize;
        let len_q = seqlens_q[i + 1] as usize - ptr_q;
        let len_k = seqlens_k[i + 1] as usize - ptr_k;

        assert!(len_q <= max_seqlen_q);
        assert!(len_k <= max_seqlen_k);

        let q = q.i((ptr_q..ptr_q + len_q, .., ..))?;
        let k = k.i((ptr_k..ptr_k + len_k, .., ..))?;
        let v = v.i((ptr_k..ptr_k + len_k, .., ..))?;

        let device = q.device();

        let (max_seqlen_q, seqlens_q) = to_offsets(&vec![len_q], device);
        let (max_seqlen_k, seqlens_k) = to_offsets(&vec![len_k], device);

        let attn = candle_flash_attn::flash_attn_varlen(
            &q,
            &k,
            &v,
            &seqlens_q,
            &seqlens_k,
            max_seqlen_q,
            max_seqlen_k,
            softmax_scale,
            causal,
        )?;

        attns.push(attn);
    }

    let attn = Tensor::cat(&attns, 0)?;
    Ok(attn)
}

impl CausalSelfAttention {
    fn forward(&self, x: &Tensor, batch_info: &BatchInfo, block_idx: usize) -> Result<Tensor> {
        let (b_sz, seq_len, hidden_size) = x.dims3()?;
        assert!(b_sz == 1);

        batch_info.log_tensor("x", &x);

        let trace = get_trace() && block_idx <= 1;

        if trace {
            println!("block #{block_idx}");
        }

        let q = self.q_proj.forward(x)?;
        let k = self.k_proj.forward(x)?;
        let v = self.v_proj.forward(x)?;

        let mut q = q.reshape((seq_len, self.num_attention_heads * self.head_dim))?;
        let mut k = k.reshape((seq_len, self.num_key_value_heads * self.head_dim))?;

        kernels::rotary_embedding(
            &batch_info.positions,
            &mut q,
            &mut k,
            self.head_dim,
            &self.cache.cos_sin,
        );

        let q = q.reshape((seq_len, self.num_attention_heads, self.head_dim))?;
        let k = k.reshape((seq_len, self.num_key_value_heads, self.head_dim))?;
        let v = v.reshape((seq_len, self.num_key_value_heads, self.head_dim))?;

        let (key_cache, value_cache) = &batch_info.kv_cache[block_idx];

        // first, stuff the query-sized key/value into the cache
        kernels::reshape_and_cache(&k, &v, key_cache, value_cache, &batch_info.slot_mapping);

        // then, extend key/value and fill them from cache
        let k = unsafe {
            kernels::unset_tensor(
                (
                    batch_info.gather_mapping.dims()[0],
                    self.num_key_value_heads,
                    self.head_dim,
                ),
                k.dtype(),
                k.device(),
            )
        };
        let v = unsafe { kernels::unset_tensor_like(&k) };
        kernels::gather_cached_kv(&k, &v, key_cache, value_cache, &batch_info.gather_mapping);

        let k = self.repeat_kv(k)?;
        let v = self.repeat_kv(v)?;

        if trace {
            println!("q2: {q:?}\n{q}");
            println!("k2: {k:?}\n{k}");
            println!("v2: {v:?}\n{v}");
        }

        let y = {
            batch_info.log_tensor("q", &q);
            batch_info.log_tensor("k", &k);
            batch_info.log_tensor("v", &v);

            // flash-attn expects (seq_len, nheads, head_dim)
            let softmax_scale = 1f32 / (self.head_dim as f32).sqrt();
            if trace {
                println!("Q {q:?} K {k:?} V {v:?}");
            }
            let causal = true;
            let y = candle_flash_attn::flash_attn_varlen(
                &q,
                &k,
                &v,
                &batch_info.seqlens_q,
                &batch_info.seqlens_k,
                batch_info.max_seqlen_q,
                batch_info.max_seqlen_k,
                softmax_scale,
                causal,
            )?;

            if DOUBLE_CHECK {
                let y2 = naive_attn(
                    &q,
                    &k,
                    &v,
                    &batch_info.seqlens_q,
                    &batch_info.seqlens_k,
                    batch_info.max_seqlen_q,
                    batch_info.max_seqlen_k,
                    softmax_scale,
                    causal,
                )?;
                check_all_close(&y, &y2, 0.1);

                let y3 = flash_attn_by_piece(
                    &q,
                    &k,
                    &v,
                    &batch_info.seqlens_q,
                    &batch_info.seqlens_k,
                    batch_info.max_seqlen_q,
                    batch_info.max_seqlen_k,
                    softmax_scale,
                    causal,
                )?;
                check_all_close(&y, &y3, 0.0001);

                let y4 = candle_flash_attn2::flash_attn_varlen(
                    &q,
                    &k,
                    &v,
                    &batch_info.seqlens_q,
                    &batch_info.seqlens_k,
                    batch_info.max_seqlen_q,
                    batch_info.max_seqlen_k,
                    softmax_scale,
                    causal,
                )?;
                check_all_close(&y, &y4, 0.0001);

                y3
            } else {
                y
            }
        };

        batch_info.log_tensor("y", &v);

        if trace {
            println!("y: {y:?}\n{y}");
        }

        let y = y.reshape(&[b_sz, seq_len, hidden_size])?;
        let y = self.o_proj.forward(&y)?;

        batch_info.log_tensor("yp", &v);

        Ok(y)
    }

    fn repeat_kv(&self, x: Tensor) -> Result<Tensor> {
        let n_rep = self.num_attention_heads / self.num_key_value_heads;
        if n_rep == 1 {
            Ok(x)
        } else {
            let (b_sz, n_kv_head, seq_len, head_dim) = x.dims4()?;
            let _x = x
                .unsqueeze(2)?
                .expand((b_sz, n_kv_head, n_rep, seq_len, head_dim))?
                .reshape((b_sz, n_kv_head * n_rep, seq_len, head_dim))?;
            todo!("dims are wrong")
        }
    }

    fn load(vb: VarBuilder, cache: &Cache, cfg: &ModelConfig) -> Result<Self> {
        let size_in = cfg.hidden_size;
        let size_q = (cfg.hidden_size / cfg.num_attention_heads) * cfg.num_attention_heads;
        let size_kv = (cfg.hidden_size / cfg.num_attention_heads) * cfg.num_key_value_heads;
        let q_proj = linear_no_bias(size_in, size_q, vb.pp("q_proj"))?;
        let k_proj = linear_no_bias(size_in, size_kv, vb.pp("k_proj"))?;
        let v_proj = linear_no_bias(size_in, size_kv, vb.pp("v_proj"))?;
        let o_proj = linear_no_bias(size_q, size_in, vb.pp("o_proj"))?;
        Ok(Self {
            q_proj,
            k_proj,
            v_proj,
            o_proj,
            num_attention_heads: cfg.num_attention_heads,
            num_key_value_heads: cfg.num_key_value_heads,
            head_dim: cfg.hidden_size / cfg.num_attention_heads,
            cache: cache.clone(),
        })
    }
}

struct Mlp {
    c_fc1: Linear,
    c_fc2: Linear,
    c_proj: Linear,
}

impl Mlp {
    fn forward(&self, x: &Tensor, batch_info: &BatchInfo) -> Result<Tensor> {
        // println!("fc1: {:?}", self.c_fc1);
        let m1 = self.c_fc1.forward(x)?;
        let m2 = self.c_fc2.forward(x)?;
        batch_info.log_tensor("w1", self.c_fc1.weight());
        batch_info.log_tensor("m1", &m1);
        batch_info.log_tensor("m2", &m2);
        let si = candle_nn::ops::silu(&m1)?;
        batch_info.log_tensor("si", &m2);
        let x = (si * &m2)?;
        self.c_proj.forward(&x)
    }

    fn load(vb: VarBuilder, cfg: &ModelConfig) -> Result<Self> {
        let h_size = cfg.hidden_size;
        let i_size = cfg.intermediate_size;
        let c_fc1 = linear_no_bias(h_size, i_size, vb.pp("gate_proj"))?;
        let c_fc2 = linear_no_bias(h_size, i_size, vb.pp("up_proj"))?;
        let c_proj = linear_no_bias(i_size, h_size, vb.pp("down_proj"))?;
        Ok(Self {
            c_fc1,
            c_fc2,
            c_proj,
        })
    }
}

struct Block {
    rms_1: RmsNorm,
    attn: CausalSelfAttention,
    rms_2: RmsNorm,
    mlp: Mlp,
}

impl Block {
    fn forward(&self, x: &Tensor, batch_info: &BatchInfo, block_idx: usize) -> Result<Tensor> {
        let residual = x;
        let x = self.rms_1.forward(x)?;
        let x = (self.attn.forward(&x, batch_info, block_idx)? + residual)?;
        let residual = &x;
        batch_info.log_tensor("x0", &x);
        let x = self.rms_2.forward(&x)?;
        batch_info.log_tensor("x1", &x);
        let x = self.mlp.forward(&x, batch_info)?;
        batch_info.log_tensor("x2", &x);
        let x = (x + residual)?;
        batch_info.log_tensor("x3", &x);
        // println!("x: {}", x);
        Ok(x)
    }

    fn load(vb: VarBuilder, cache: &Cache, cfg: &ModelConfig) -> Result<Self> {
        let attn = CausalSelfAttention::load(vb.pp("self_attn"), cache, cfg)?;
        let mlp = Mlp::load(vb.pp("mlp"), cfg)?;
        let rms_norm_eps = cfg.rms_norm_eps.unwrap();
        let rms_1 = candle_nn::rms_norm(cfg.hidden_size, rms_norm_eps, vb.pp("input_layernorm"))?;
        let rms_2 = candle_nn::rms_norm(
            cfg.hidden_size,
            rms_norm_eps,
            vb.pp("post_attention_layernorm"),
        )?;
        Ok(Self {
            rms_1,
            attn,
            rms_2,
            mlp,
        })
    }
}

pub struct Llama {
    wte: Embedding,
    blocks: Vec<Block>,
    ln_f: RmsNorm,
    lm_head: Linear,
}

impl Llama {
    pub fn forward(&self, batch_info: &BatchInfo) -> Result<Tensor> {
        let mut x = self.wte.forward(&batch_info.tokens)?.unsqueeze(0)?;
        for (block_idx, block) in self.blocks.iter().enumerate() {
            x = block.forward(&x, batch_info, block_idx)?;
        }
        let x0 = self.ln_f.forward(&x)?;
        // println!("x: {}", x0);

        // skip first zero
        let mut idx = batch_info.seqlens_q.i(1..)?;
        // subtract 1 from each index
        idx = idx.sub(&Tensor::ones_like(&idx)?)?;
        let x = x0.i((.., &idx, ..))?;
        // println!("x0 {:?} x {:?} idx {}", x0, x, idx);

        let logits = self.lm_head.forward(&x)?.squeeze(0)?;
        logits.to_dtype(DType::F32)
    }

    pub fn load(vb: VarBuilder, cfg: &ModelConfig) -> Result<Self> {
        let bar = indicatif::ProgressBar::new(cfg.num_hidden_layers as u64);
        bar.set_style(
            indicatif::ProgressStyle::with_template(
                "[{elapsed_precise}] {bar:60.cyan/blue} {pos:>4}/{len:4} [{eta_precise}] {msg}",
            )
            .unwrap(),
        );

        let cache = Cache::new(cfg.get_dtype(), cfg, vb.device())?;
        let wte = embedding(cfg, vb.pp("model.embed_tokens"))?;
        let lm_head = linear_no_bias(cfg.hidden_size, cfg.vocab_size, vb.pp("lm_head"))?;
        let ln_f = candle_nn::rms_norm(
            cfg.hidden_size,
            cfg.rms_norm_eps.unwrap(),
            vb.pp("model.norm"),
        )?;
        let blocks: Vec<_> = (0..cfg.num_hidden_layers)
            .map(|i| {
                bar.inc(1);
                if bar.is_hidden() {
                    eprint!(".");
                }
                // log::info!("loading block {}/{}", i, cfg.num_hidden_layers);
                Block::load(vb.pp(&format!("model.layers.{i}")), &cache, cfg).unwrap()
            })
            .collect();

        if bar.is_hidden() {
            eprintln!(" done");
        }
        bar.finish();

        Ok(Self {
            wte,
            blocks,
            ln_f,
            lm_head,
        })
    }
}