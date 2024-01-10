/// <reference path="./native.d.ts" />
import { TokenSet, tokenize, detokenize, RegexConstraint, CfgConstraint, SubStrConstraint, Constraint, get_var, set_var, append_var, eos_token, } from "_aici";
export { TokenSet, tokenize, detokenize, RegexConstraint, CfgConstraint, SubStrConstraint, Constraint, get_var, set_var, append_var, eos_token, };
import * as _aici from "_aici";
function dbgarg(arg, depth) {
    const maxElts = 20;
    const maxDepth = 2;
    const maxStr = 200;
    if (arg === null)
        return "null";
    if (arg === undefined)
        return "undefined";
    if (typeof arg === "object") {
        if (Array.isArray(arg)) {
            if (depth >= maxDepth && arg.length > 0)
                return "[...]";
            let suff = "]";
            if (arg.length > maxElts) {
                arg = arg.slice(0, maxElts);
                suff = ", ...]";
            }
            return "[" + arg.map((x) => dbgarg(x, depth + 1)).join(", ") + suff;
        }
        else {
            let keys = Object.keys(arg);
            if (depth >= maxDepth && keys.length > 0)
                return "{...}";
            let suff = "}";
            if (keys.length > maxElts) {
                suff = ", ...}";
                keys = keys.slice(0, maxElts);
            }
            return ("{" +
                keys.map((k) => `${k}: ${dbgarg(arg[k], depth + 1)}`).join(", ") +
                suff);
        }
    }
    else {
        if (depth === 0 || typeof arg !== "string") {
            return arg.toString();
        }
        else {
            const r = arg.toString();
            if (r.length > maxStr) {
                return r.substring(0, maxStr) + "...";
            }
            else
                return r;
        }
    }
}
export function inspect(v) {
    return dbgarg(v, 0);
}
export function log(...args) {
    console._print(args.map((x) => inspect(x)).join(" "));
}
console.log = log;
console.info = log;
console.warn = log;
console.debug = log;
console.trace = log;
export class AssertionError extends Error {
}
function assert(cond, msg = "Assertion failed") {
    if (!cond)
        throw new AssertionError("Assertion failed");
}
/**
 * Get list of tokens in the current sequence, including the prompt.
 */
export function get_tokens() {
    assert(!!AiciAsync.instance);
    return AiciAsync.instance._tokens;
}
/**
 * Get the length of the prompt in the current sequence.
 */
export function get_prompt_len() {
    assert(!!AiciAsync.instance);
    return AiciAsync.instance._prompt_len;
}
export class MidProcessResult {
    constructor() {
        this._skipMe = false;
        this._n_stop = false;
        this._n_logit_bias = null;
        this._n_backtrack = 0;
        this._n_ff_tokens = [];
    }
    static stop() {
        const res = new MidProcessResult();
        res._n_stop = true;
        return res;
    }
    static skipMe() {
        const res = new MidProcessResult();
        res._skipMe = true;
        return res;
    }
    static bias(bias) {
        const res = new MidProcessResult();
        res._n_logit_bias = bias;
        return res;
    }
    static splice(backtrack, tokens) {
        const res = new MidProcessResult();
        assert(backtrack >= 0);
        assert(Array.isArray(tokens));
        res._n_backtrack = backtrack;
        res._n_ff_tokens = tokens;
        return res;
    }
}
export class PreProcessResult {
    constructor() {
        this._n_suspended = false;
        this._n_ff_tokens = [];
        this._n_attention_masks = [[]];
    }
    static continue_() {
        return new PreProcessResult();
    }
    static suspend() {
        const res = new PreProcessResult();
        res._n_suspended = true;
        return res;
    }
    static fork(num_forks) {
        const res = new PreProcessResult();
        res._n_attention_masks = Array.from({ length: num_forks }, () => []);
        return res;
    }
    static ff_tokens_pre(toks) {
        const res = new PreProcessResult();
        res._n_ff_tokens = toks;
        return res;
    }
}
export class PostProcessResult {
    constructor(stop_seq = false) {
        this._n_stop_seq = stop_seq;
    }
    static continue_() {
        return new PostProcessResult();
    }
    static stop() {
        return new PostProcessResult(true);
    }
    static from_tokens(tokens) {
        return new PostProcessResult(tokens.includes(eos_token()));
    }
}
export class NextToken {
    constructor() {
        this.finished = false;
        this.curr_tokens = null;
        this.fork_group = [];
    }
    /**
     * Awaiting this will return generated token (or tokens, if fast-forwarding requested by self.mid_process()).
     * You have only ~1ms to process the results before awaiting a new instance of NextToken() again.
     */
    run() {
        assert(!this._resolve);
        AiciAsync.instance._nextToken(this);
        return new Promise((resolve) => {
            this._resolve = resolve;
        });
    }
    /**
     * Override to suspend, if the model cannot continue generating tokens
     * now (for example, not all variables are available to compute bias).
     * ~1ms time limit.
     */
    pre_process() {
        return PreProcessResult.continue_();
    }
    /**
     * This can be overridden to return a bias, fast-forward tokens, backtrack etc.
     * ~20ms time limit.
     */
    mid_process() {
        return MidProcessResult.bias(new TokenSet());
    }
    /**
     * This can be overridden to do something with generated tokens.
     * ~1ms time limit.
     * @param tokens tokens generated in the last step
     */
    post_process(tokens) {
        return PostProcessResult.continue_();
    }
    //
    // Internal methods
    //
    _pre_process() {
        this.reset();
        return this.pre_process();
    }
    _mid_process(fork_group) {
        this.fork_group = fork_group;
        return this.mid_process();
    }
    _post_process(_backtrack, tokens) {
        this.curr_tokens = tokens;
        this.finished = tokens.includes(eos_token());
        return this.post_process(tokens);
    }
    reset() {
        this.curr_tokens = null;
        this.fork_group = [];
    }
}
/**
 * Forces next tokens to be exactly the given text.
 */
export async function fixed(text) {
    await new FixedTokens(text).run();
    console.log("RUN done");
}
/**
 * Forces next tokens to be exactly the given text.
 * If following is given, the text replaces everything that follows the label.
 */
export class FixedTokens extends NextToken {
    constructor(text, following = null) {
        super();
        this.fixed_tokens = tokenize(text);
        this.following = following;
    }
    pre_process() {
        if (this.following === null) {
            return PreProcessResult.ff_tokens_pre(this.fixed_tokens);
        }
        return PreProcessResult.continue_();
    }
    mid_process() {
        let backtrack = 0;
        if (this.following !== null) {
            backtrack = get_tokens().length - this.following.ptr;
            assert(backtrack >= 0);
            console.log("backtrack", backtrack);
        }
        return MidProcessResult.splice(backtrack, this.fixed_tokens);
    }
}
/**
 * Indicates that the generation should stop.
 */
export class StopToken extends NextToken {
    constructor() {
        super();
    }
    mid_process() {
        return MidProcessResult.stop();
    }
    post_process(_tokens) {
        this.finished = false; // we're never finished, just keep yelling STOP!
        return PostProcessResult.stop();
    }
}
/**
 * Generates a token that satisfies the given constraint.
 * The constraint will be constructed in mid_process() phase, which has slightly longer time limit.
 */
export class ConstrainedToken extends NextToken {
    constructor(mk_constraint) {
        super();
        this.mk_constraint = mk_constraint;
        this._constraint = null;
    }
    mid_process() {
        const bias = new TokenSet();
        if (this._constraint === null) {
            this._constraint = this.mk_constraint();
        }
        this._constraint.allow_tokens(bias);
        return MidProcessResult.bias(bias);
    }
    post_process(tokens) {
        const c = this._constraint;
        assert(!!c);
        tokens.forEach((t) => c.append_token(t));
        if (c.eos_forced()) {
            this.finished = true;
        }
        return PostProcessResult.continue_();
    }
}
export class PreToken extends NextToken {
    mid_process() {
        return MidProcessResult.skipMe();
    }
}
class _Fork extends PreToken {
    constructor(num_forks) {
        super();
        this.num_forks = num_forks;
    }
    pre_process() {
        return PreProcessResult.fork(this.num_forks);
    }
}
/**
 * Forks the execution into `num_forks` branches.
 * @param num_forks how many branches
 * @returns a number from 0 to `num_forks`-1, indicating the branch
 */
export async function fork(num_forks) {
    const f = new _Fork(num_forks);
    await f.run();
    return f.fork_group.indexOf(_aici.self_seq_id());
}
class _WaitVars extends PreToken {
    constructor(vars) {
        super();
        this.vars = vars;
        this.values = [];
    }
    pre_process() {
        const values = this.vars.map((v) => get_var(v));
        if (values.includes(null)) {
            return PreProcessResult.suspend();
        }
        this.values = values;
        return PreProcessResult.continue_();
    }
}
/**
 * Suspends execution until all variables are available.
 * @param vars names of variables
 * @returns values of the variables
 */
export async function waitVars(...vars) {
    const w = new _WaitVars(vars);
    await w.run();
    return w.values;
}
/**
 * Awaiting this returns the prompt passed by the user.
 * The code before call to this function has a long time limit (~1000ms).
 * Afterwards, the time limit is ~1ms before awaiting NextToken().
 */
export function getPrompt() {
    return new GetPrompt().run();
}
class GetPrompt {
    run() {
        assert(!this._resolve);
        return new Promise((resolve) => {
            AiciAsync.instance._setGetPrompt(this);
            this._resolve = resolve;
        });
    }
}
export class AiciAsync {
    _setGetPrompt(g) {
        assert(!this._getPrompt);
        assert(!this._token);
        assert(g instanceof GetPrompt);
        this._getPrompt = g;
    }
    _nextToken(t) {
        assert(!this._token);
        assert(!this._getPrompt);
        assert(t instanceof NextToken);
        this._token = t;
    }
    constructor(f) {
        this._tokens = [];
        this._prompt_len = 0;
        this._fork_group = [];
        assert(!AiciAsync.instance);
        AiciAsync.instance = this;
        globalThis._aici_cb = this;
        this.init_prompt = this.init_prompt.bind(this);
        this.pre_process = this.pre_process.bind(this);
        this.mid_process = this.mid_process.bind(this);
        this.post_process = this.post_process.bind(this);
        f().then(async () => {
            console.log("JSVM: done");
            while (true) {
                await new StopToken().run();
            }
        });
        if (this._getPrompt) {
            assert(this._getPrompt instanceof GetPrompt);
            assert(!this._token);
        }
        else {
            assert(this._token instanceof NextToken);
        }
    }
    step(tokens) {
        if (this._pending_cb != null) {
            // TODO
            this._token = this._pending_cb;
            this._pending_cb = undefined;
            return;
        }
        const nextToken = this._token;
        assert(nextToken instanceof NextToken);
        const resolve = nextToken._resolve;
        assert(!!resolve);
        // console.log("reset");
        this._token = undefined;
        nextToken._resolve = undefined;
        resolve(tokens);
        // console.log("t2", this._token, resolve);
        // this happens only in the deferred jobs...
        // assert((this._token as any) instanceof NextToken);
    }
    init_prompt(prompt) {
        assert(!this._tokens.length);
        this._prompt_len = prompt.length;
        this._tokens.push(...prompt);
        if (this._getPrompt) {
            this._getPrompt._resolve(prompt);
            this._getPrompt = undefined;
        }
        assert(this._token instanceof NextToken);
    }
    pre_process() {
        // console.log("tok", this._token);
        assert(this._token instanceof NextToken);
        if (this._token.finished) {
            this._token = new StopToken();
        }
        const r = this._token._pre_process();
        assert(r instanceof PreProcessResult);
        return r;
    }
    mid_process(fork_group) {
        assert(this._token instanceof NextToken);
        let r = this._token._mid_process(fork_group);
        assert(r instanceof MidProcessResult);
        while (r._skipMe) {
            this.step([]); // TODO
            assert(this._token instanceof NextToken);
            const r2 = this._token._pre_process();
            assert(r2 instanceof PreProcessResult);
            assert(r2._n_attention_masks.length === 1, "nested fork not allowed");
            if (r2._n_suspended) {
                // Need to generate one fake token...
                this._pending_cb = this._token;
                const f = new FixedTokens("░");
                assert(f.fixed_tokens.length === 1);
                this._token = f;
            }
            r = this._token._mid_process(fork_group);
            assert(r instanceof MidProcessResult);
        }
        assert(Array.isArray(r._n_ff_tokens));
        return r;
    }
    post_process(backtrack, tokens) {
        if (backtrack > 0) {
            this._tokens.splice(-backtrack);
        }
        this._tokens.push(...tokens);
        assert(this._token instanceof NextToken);
        const r = this._token._post_process(backtrack, tokens.slice());
        assert(r instanceof PostProcessResult);
        this.step(tokens);
        return r;
    }
}
/**
 * Starts the AICI loop. The coroutine may first `await aici.getPrompt()` and
 * then can `await aici.gen_*()` or `await aici.FixedTokens()` multiple times.
 * @param f async function
 */
export function start(f) {
    return new AiciAsync(f);
}
/**
 * Runs the loop as a test.
 */
export function test(f) {
    return new AiciAsync(() => f().then(() => {
        console.log("TEST OK");
    }));
}
export class Label {
    /**
     * Create a new label the indictes the current position in the sequence.
     * Can be passed as `following=` argument to `FixedTokens()`.
     */
    constructor() {
        this.ptr = get_tokens().length;
    }
    /**
     * Return tokens generated since the label.
     */
    tokens_since() {
        return get_tokens().slice(this.ptr);
    }
    /**
     * Return text generated since the label.
     */
    text_since() {
        return detokenize(this.tokens_since()).toString();
    }
}
export class ChooseConstraint extends Constraint {
    constructor(options) {
        super();
        this.ptr = 0;
        this.options = options.map((o) => tokenize(o));
    }
    eos_allowed() {
        return this.options.some((o) => o.length === this.ptr);
    }
    eos_forced() {
        return this.options.length === 1 && this.options[0].length === this.ptr;
    }
    token_allowed(t) {
        return this.options.some((o) => this.ptr < o.length && o[this.ptr] === t);
    }
    append_token(t) {
        this.options = this.options.filter((o) => this.ptr < o.length && o[this.ptr] === t);
        this.ptr += 1;
    }
    allow_tokens(ts) {
        for (const o of this.options) {
            if (this.ptr < o.length) {
                ts.add(o[this.ptr]);
            }
            else if (this.ptr === o.length) {
                ts.add(eos_token());
            }
        }
    }
}
export async function gen_tokens(options) {
    const res = [];
    const { regex, yacc, substring, substring_end = '"', options: optionList, store_var, stop_at, max_tokens = 20, } = options;
    let constraint;
    assert([regex, substring, yacc, optionList].filter((x) => x !== undefined)
        .length <= 1);
    if (regex !== undefined) {
        constraint = new RegexConstraint(regex);
    }
    else if (substring !== undefined) {
        constraint = new SubStrConstraint(substring, substring_end);
    }
    else if (yacc !== undefined) {
        constraint = new CfgConstraint(yacc);
    }
    else if (optionList !== undefined) {
        constraint = new ChooseConstraint(optionList);
    }
    else {
        constraint = new Constraint();
    }
    const next_token = new ConstrainedToken(() => constraint);
    for (let i = 0; i < max_tokens; i++) {
        const tokens = await next_token.run();
        res.push(...tokens);
        const text = detokenize(res).toString();
        if (stop_at !== undefined && text.includes(stop_at)) {
            break;
        }
        if (next_token.finished) {
            break;
        }
    }
    if (store_var !== undefined) {
        set_var(store_var, detokenize(res));
    }
    console.log("GEN", res, detokenize(res).toString());
    return res;
}
export async function gen_text(options) {
    const tokens = await gen_tokens(options);
    return detokenize(tokens).toString();
}
export function check_var(name, value) {
    const v = get_var(name);
    if (v == null) {
        throw new AssertionError(`Variable ${name} is unset`);
    }
    const vStr = v.toString();
    if (vStr !== value) {
        throw new AssertionError(`Variable ${name}: ${vStr} != ${value}`);
    }
}
export function check_vars(d) {
    for (const [k, v] of Object.entries(d)) {
        check_var(k, v);
    }
}
