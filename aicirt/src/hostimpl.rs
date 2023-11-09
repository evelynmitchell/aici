use aici_abi::{
    bytes::{clone_vec_as_bytes, vec_from_bytes, TokRxInfo},
    TokenId,
};
use anyhow::{anyhow, Result};
use log::{info, warn};
use std::{rc::Rc, sync::Arc};
use tokenizers::Tokenizer;

use crate::{
    shm::Shm,
    worker::{ExecOp, GroupCmd, GroupHandle, GroupResp},
};

pub type ModuleInstId = usize;

#[derive(Debug, Clone)]
pub struct AiciLimits {
    pub max_memory_bytes: usize,
    pub max_step_ms: u64,
    pub max_init_ms: u64,
    pub logit_memory_bytes: usize,
}

// this is available to functions called from wasm
pub struct ModuleData {
    pub id: ModuleInstId,
    log: Vec<u8>,
    printed_log: usize,
    pub globals: GlobalInfo,
    pub group_channel: GroupHandle,
    pub process_result: Vec<u8>,
    pub logit_ptr: &'static mut [f32],
    pub linker: Arc<wasmtime::Linker<ModuleData>>,
    pub instance: Option<wasmtime::Instance>,
    pub memory: Option<wasmtime::Memory>,
    pub module: wasmtime::Module,
    tokenizer: Option<Tokenizer>,
    pub store_limits: wasmtime::StoreLimits,
    pub had_error: bool,
    blobs: Vec<Rc<Vec<u8>>>,
}

const MAXLOG: usize = 32 * 1024;

pub const LOGIT_BIAS_ALLOW: f32 = 1.0e10;
pub const LOGIT_BIAS_DISALLOW: f32 = 0.0;

pub struct BlobId(u32);

impl BlobId {
    pub const MODULE_ARG: BlobId = BlobId(1);
    pub const TOKENIZE: BlobId = BlobId(2);
    pub const TOKENS: BlobId = BlobId(3);
    pub const PROCESS_ARG: BlobId = BlobId(4);
    pub const STORAGE_RESULT: BlobId = BlobId(5);

    pub const MAX_BLOB_ID: u32 = 20;

    // these have special handling:
    pub const TRIE: BlobId = BlobId(100);
}

impl ModuleData {
    pub fn new(
        id: ModuleInstId,
        limits: &AiciLimits,
        module: &wasmtime::Module,
        module_arg: String,
        linker: &Arc<wasmtime::Linker<ModuleData>>,
        globals: GlobalInfo,
        group_channel: GroupHandle,
    ) -> Self {
        let store_limits = wasmtime::StoreLimitsBuilder::new()
            .memories(1)
            .memory_size(limits.max_memory_bytes)
            .tables(2)
            .table_elements(100000)
            .instances(1)
            .trap_on_grow_failure(true)
            .build();
        let mut r = ModuleData {
            id,
            log: Vec::new(),
            printed_log: 0,
            globals,
            group_channel,
            module: module.clone(),
            linker: linker.clone(),
            instance: None,
            memory: None,
            tokenizer: None,
            store_limits,
            process_result: Vec::new(),
            logit_ptr: &mut [],
            had_error: false,
            blobs: vec![Rc::new(Vec::new()); BlobId::MAX_BLOB_ID as usize],
        };
        r.set_blob(BlobId::MODULE_ARG, module_arg.as_bytes().to_vec());
        r
    }

    fn clear_blob(&mut self, blob_id: BlobId) {
        self.set_blob(blob_id, vec![])
    }

    fn set_blob(&mut self, blob_id: BlobId, bytes: Vec<u8>) {
        self.blobs[blob_id.0 as usize] = Rc::new(bytes);
    }

    pub fn set_process_arg(&mut self, bytes: Vec<u8>) {
        self.process_result.clear();
        self.set_blob(BlobId::PROCESS_ARG, bytes);
    }

    pub fn set_exec_data(&mut self, data: ExecOp, shm: &Shm) {
        self.set_process_arg(data.op);
        let nument = self.globals.tokrx_info.vocab_size as usize;
        let ptr = shm.ptr_at(data.logit_offset);
        assert!(LOGIT_BIAS_DISALLOW == 0.0);
        unsafe {
            std::ptr::write_bytes(ptr, 0, nument * 4);
            self.logit_ptr = std::slice::from_raw_parts_mut(ptr as *mut f32, nument);
        }
    }

    pub fn tokenize(&mut self, s: &str) -> Result<Vec<u32>> {
        if self.tokenizer.is_none() {
            let info = &self.globals;
            let tok = Tokenizer::from_bytes(info.hf_tokenizer_bytes).unwrap();
            self.tokenizer = Some(tok);
        };
        let tokens = self.tokenizer.as_ref().unwrap().encode(s, false);
        match tokens {
            Err(e) => Err(anyhow!(e)),
            Ok(tokens) => Ok(Vec::from(tokens.get_ids())),
        }
    }

    pub fn fatal(&mut self, msg: &str) {
        warn!("{}: fatal error {}", self.id, msg);
        let msg = format!("FATAL ERROR: {}\n", msg);
        self.write_log(msg.as_bytes());
        self.had_error = true;
        // ideally, this should call into the module and cause panic
    }

    pub fn warn(&mut self, msg: &str) {
        warn!("{}: {}", self.id, msg);
        let msg = format!("warning: {}\n", msg);
        self.write_log(msg.as_bytes());
    }

    pub fn write_log(&mut self, bytes: &[u8]) {
        self.log.extend_from_slice(bytes);
        if self.log.len() > MAXLOG {
            let drop = MAXLOG / 4;
            if self.had_error {
                // normally, we drop prefix, but if "had_error" is set
                // we drop the suffix instead to avoid flushing out "FATAL ERROR" message
                self.log.truncate(self.log.len() - drop);
            } else {
                self.printed_log = self.printed_log.saturating_sub(drop);
                self.log.drain(0..drop);
            }
        }
    }

    pub fn string_log(&mut self) -> String {
        self.printed_log = 0;
        let logs = String::from_utf8_lossy(&self.log).to_string();
        self.log.clear();
        logs
    }

    pub fn flush_logs(&mut self, name: &str) {
        if !log::log_enabled!(log::Level::Info) {
            return;
        }

        let data = &self.log[self.printed_log..];
        if data.len() == 0 {
            return;
        }

        let logs = String::from_utf8_lossy(data).to_string();
        self.printed_log = self.log.len();

        for line in logs.lines() {
            info!("{}:{}> {}", self.id, name, line);
        }
    }

    pub fn aici_host_storage_cmd(&mut self, m: Vec<u8>) -> BlobId {
        self.clear_blob(BlobId::STORAGE_RESULT);
        match serde_json::from_slice(&m) {
            Ok(cmd) => {
                let res = self.group_channel.send_cmd(GroupCmd::StorageCmd { cmd });
                match res {
                    Ok(GroupResp::StorageResp { resp }) => {
                        let res_bytes = serde_json::to_vec(&resp).unwrap();
                        self.set_blob(BlobId::STORAGE_RESULT, res_bytes);
                    }
                    Ok(r) => self.fatal(&format!("storage_cmd invalid resp: {r:?}")),
                    Err(msg) => self.fatal(&format!("storage_cmd send error: {msg:?}")),
                }
            }
            Err(e) => self.fatal(&format!("storage_cmd error: {e:?}")),
        }
        BlobId::STORAGE_RESULT
    }
}

#[derive(Clone)]
pub struct GlobalInfo {
    pub tokrx_info: TokRxInfo,
    pub trie_bytes: Arc<Vec<u8>>,
    pub hf_tokenizer_bytes: &'static [u8],
}

fn check_fatal(caller: &mut wasmtime::Caller<'_, ModuleData>) {
    if caller.data().had_error {
        fatal_error(caller, "see above")
    }
}

fn fatal_error(caller: &mut wasmtime::Caller<'_, ModuleData>, msg: &str) {
    caller.data_mut().fatal(msg);
    match caller.get_export("aici_panic") {
        Some(wasmtime::Extern::Func(f)) => {
            let mut res = Vec::new();
            let _ = f.call(caller, &[], &mut res);
        }
        _ => {}
    }
}

fn read_caller_mem(caller: &wasmtime::Caller<'_, ModuleData>, ptr: u32, len: u32) -> Vec<u8> {
    let mem = caller.data().memory.unwrap();
    let ptr = ptr as usize;
    Vec::from(&mem.data(&caller)[ptr..(ptr + len as usize)])
}

fn write_caller_mem(
    caller: &mut wasmtime::Caller<'_, ModuleData>,
    ptr: u32,
    len: u32,
    src: &[u8],
) -> u32 {
    if len > 0 {
        let mem = caller.data().memory.unwrap();
        let min_len = std::cmp::min(len as usize, src.len());
        mem.write(caller, ptr as usize, &src[..min_len]).unwrap();
    }
    src.len() as u32
}

pub fn setup_linker(engine: &wasmtime::Engine) -> Result<Arc<wasmtime::Linker<ModuleData>>> {
    let mut linker = wasmtime::Linker::<ModuleData>::new(engine);
    linker.func_wrap(
        "env",
        "aici_host_print",
        |mut caller: wasmtime::Caller<'_, ModuleData>, ptr: u32, len: u32| {
            let m = read_caller_mem(&caller, ptr, len);
            caller.data_mut().write_log(&m);
        },
    )?;

    linker.func_wrap(
        "env",
        "aici_host_read_blob",
        |mut caller: wasmtime::Caller<'_, ModuleData>, blob_id: u32, ptr: u32, len: u32| {
            if blob_id == BlobId::TRIE.0 {
                let trie_bytes = caller.data().globals.trie_bytes.clone();
                write_caller_mem(&mut caller, ptr, len, &trie_bytes)
            } else if blob_id < BlobId::MAX_BLOB_ID {
                let blob = caller.data().blobs[blob_id as usize].clone();
                write_caller_mem(&mut caller, ptr, len, &blob)
            } else {
                fatal_error(&mut caller, "invalid blob_id");
                0
            }
        },
    )?;

    linker.func_wrap("env", "aici_host_module_arg", || BlobId::MODULE_ARG.0)?;
    linker.func_wrap("env", "aici_host_process_arg", || BlobId::PROCESS_ARG.0)?;
    linker.func_wrap("env", "aici_host_token_trie", || BlobId::TRIE.0)?;
    linker.func_wrap("env", "aici_host_tokens", || BlobId::TOKENS.0)?;

    // uint32_t aici_host_tokenize(const uint8_t *src, uint32_t src_size, uint32_t *dst, uint32_t dst_size);
    linker.func_wrap(
        "env",
        "aici_host_tokenize",
        |mut caller: wasmtime::Caller<'_, ModuleData>, src: u32, src_size: u32| {
            let m = read_caller_mem(&caller, src, src_size);
            let s = String::from_utf8_lossy(&m);
            let tokens = caller.data_mut().tokenize(&s);
            match tokens {
                Err(e) => {
                    caller.data_mut().warn(&format!("tokenize error: {e:?}"));
                    caller.data_mut().clear_blob(BlobId::TOKENIZE);
                }
                Ok(tokens) => {
                    caller
                        .data_mut()
                        .set_blob(BlobId::TOKENIZE, clone_vec_as_bytes(&tokens));
                }
            }
            BlobId::TOKENIZE.0
        },
    )?;

    linker.func_wrap(
        "env",
        "aici_host_return_logit_bias",
        |mut caller: wasmtime::Caller<'_, ModuleData>, src: u32| {
            let numtok = caller.data().globals.tokrx_info.vocab_size as usize;
            let numbytes = 4 * ((numtok + 31) / 32);
            if caller.data().logit_ptr.len() == 0 {
                return fatal_error(&mut caller, "logit_ptr is empty");
            }
            let m = read_caller_mem(&caller, src, numbytes as u32);
            let masks = vec_from_bytes::<u32>(&m);
            let ptr = &mut caller.data_mut().logit_ptr;
            info!(
                "return_logits: numtok={} numbytes={} mlen={} maskslen={}",
                numtok,
                numbytes,
                m.len(),
                masks.len()
            );
            for idx in 0..numtok {
                let mask = masks[idx / 32];
                let bit = 1 << (idx % 32);
                if mask & bit != 0 {
                    ptr[idx] = LOGIT_BIAS_ALLOW;
                }
            }
        },
    )?;

    linker.func_wrap(
        "env",
        "aici_host_self_seq_id",
        |caller: wasmtime::Caller<'_, ModuleData>| caller.data().id as u32,
    )?;

    linker.func_wrap(
        "env",
        "aici_host_return_process_result",
        |mut caller: wasmtime::Caller<'_, ModuleData>, src: u32, src_size: u32| {
            let m = read_caller_mem(&caller, src, src_size);
            caller.data_mut().process_result = m;
        },
    )?;

    linker.func_wrap(
        "env",
        "aici_host_storage_cmd",
        |mut caller: wasmtime::Caller<'_, ModuleData>, src: u32, src_size: u32| {
            let m = read_caller_mem(&caller, src, src_size);
            let r = caller.data_mut().aici_host_storage_cmd(m);
            check_fatal(&mut caller);
            r.0
        },
    )?;

    let linker = Arc::new(linker);
    Ok(linker)
}
