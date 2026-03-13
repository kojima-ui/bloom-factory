//! SignatureCipher decoding for YouTube stream URLs.
//!
//! Modern YouTube player JS (player_es6.vflset / player_ias.vflset, 2024+) uses a
//! dispatch function `fQ` with a helper object `uC` instead of the classic
//! `a=a.split("");...; return a.join("")` pattern.
//!
//! Extraction algorithm:
//!   1. Parse the `h[]` string-constant array from the player JS.
//!   2. Find `var uC={...}` and classify each method as Splice/Reverse/Swap
//!      by examining which h[N] indices appear in its body.
//!   3. Find the outer cipher call `fQ(D, X, fQ(.., m.s))` → V = X ^ D.
//!   4. Find and parse the `if((D|80)==D){...}` block inside `fQ` to get the
//!      ordered list of `uC[h[V^CONST]](x, ...)` calls.
//!   5. Translate those calls to a `CipherOp` sequence.
//!
//! Legacy fallback:
//!   When the fQ pattern is absent (older player JS), the classic
//!   `a=a.split(""); ...; return a.join("")` extraction is attempted.

use bex_core::resolver::component::content_resolver::utils;
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Cipher operations
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum CipherKind {
    Reverse,
    Splice,
    Swap,
}

#[derive(Debug, Clone)]
struct CipherOp {
    kind: CipherKind,
    index: usize,
}

/// A cipher manifest extracted from YouTube's player JS.
#[derive(Debug, Clone)]
pub struct CipherManifest {
    ops: Vec<CipherOp>,
    pub sig_timestamp: String,
}

impl CipherManifest {
    /// Apply all operations to decode a scrambled signature.
    pub fn decipher(&self, sig: &str) -> String {
        let mut chars: Vec<char> = sig.chars().collect();
        for op in &self.ops {
            match op.kind {
                CipherKind::Reverse => {
                    chars.reverse();
                }
                CipherKind::Splice => {
                    if op.index < chars.len() {
                        chars = chars[op.index..].to_vec();
                    }
                }
                CipherKind::Swap => {
                    if !chars.is_empty() {
                        let idx = op.index % chars.len();
                        chars.swap(0, idx);
                    }
                }
            }
        }
        chars.into_iter().collect()
    }
}

// ---------------------------------------------------------------------------
// Player JS fetching & cipher extraction
// ---------------------------------------------------------------------------

const STORAGE_KEY_PLAYER_JS: &str = "ytm_player_js_url";
const STORAGE_KEY_SIG_TIMESTAMP: &str = "ytm_sig_timestamp";
const STORAGE_KEY_CIPHER_OPS: &str = "ytm_cipher_ops";

/// Fetch and parse a CipherManifest from YouTube's player JS.
/// Uses plugin storage to cache the player JS URL and extracted ops.
pub fn get_cipher_manifest() -> Result<CipherManifest, String> {
    // Try to load cached cipher ops from storage.
    if let (Some(ops_json), Some(sig_ts)) = (
        utils::storage_get(STORAGE_KEY_CIPHER_OPS),
        utils::storage_get(STORAGE_KEY_SIG_TIMESTAMP),
    ) {
        if let Ok(manifest) = parse_cached_ops(&ops_json, &sig_ts) {
            return Ok(manifest);
        }
    }

    // Cache miss or corrupted — fetch fresh.
    fetch_cipher_manifest_fresh()
}

/// Invalidate cached cipher data and fetch fresh.
pub fn refresh_cipher_manifest() -> Result<CipherManifest, String> {
    let _ = utils::storage_set(STORAGE_KEY_PLAYER_JS, "");
    let _ = utils::storage_set(STORAGE_KEY_CIPHER_OPS, "");
    let _ = utils::storage_set(STORAGE_KEY_SIG_TIMESTAMP, "");
    fetch_cipher_manifest_fresh()
}

fn fetch_cipher_manifest_fresh() -> Result<CipherManifest, String> {
    // Step 1: Get the player JS URL from YouTube Music page.
    let js_url = fetch_player_js_url()?;
    let _ = utils::storage_set(STORAGE_KEY_PLAYER_JS, &js_url);

    // Step 2: Download the player JS.
    let js_content = http_get(&js_url)?;

    // Step 3: Extract signatureTimestamp.
    let sig_timestamp = extract_sig_timestamp(&js_content)
        .ok_or_else(|| "Failed to extract signatureTimestamp from player JS".to_string())?;

    // Step 4: Extract cipher operations.
    let ops = extract_cipher_ops(&js_content)?;

    // Step 5: Cache to storage.
    let ops_serialized = serialize_ops(&ops);
    let _ = utils::storage_set(STORAGE_KEY_SIG_TIMESTAMP, &sig_timestamp);
    let _ = utils::storage_set(STORAGE_KEY_CIPHER_OPS, &ops_serialized);

    Ok(CipherManifest { ops, sig_timestamp })
}

// ---------------------------------------------------------------------------
// Stream URL decoding
// ---------------------------------------------------------------------------

/// Decode a signatureCipher string into a playable URL.
///
/// The cipher string has the format: `s=ENCODED_SIG&sp=sig&url=BASE_URL`
pub fn decode_stream_url(cipher_str: &str, manifest: &CipherManifest) -> Result<String, String> {
    let parts = parse_query_string(cipher_str);

    let enc_sig = parts
        .get("s")
        .or_else(|| parts.get("sig"))
        .ok_or("Missing 's' (signature) in signatureCipher")?;
    let base_url = parts.get("url").ok_or("Missing 'url' in signatureCipher")?;
    let sp = parts.get("sp").map(|s| s.as_str()).unwrap_or("signature");

    let decoded_sig = manifest.decipher(enc_sig);
    let encoded_sig = urlencoding::encode(&decoded_sig);

    let mut url = append_or_replace_query_param(base_url, sp, &encoded_sig);

    // ytexplode also ensures ratebypass=yes to avoid throttled stream URLs.
    if !url.contains("ratebypass=") {
        url = append_or_replace_query_param(&url, "ratebypass", "yes");
    }

    Ok(url)
}

fn append_or_replace_query_param(url: &str, key: &str, value: &str) -> String {
    let mut parts = url.splitn(2, '?');
    let base = parts.next().unwrap_or(url);
    let query = parts.next().unwrap_or("");

    let mut kv_pairs = Vec::<(String, String)>::new();
    let mut replaced = false;

    for pair in query.split('&').filter(|p| !p.is_empty()) {
        if let Some((k, v)) = pair.split_once('=') {
            if k == key {
                kv_pairs.push((k.to_string(), value.to_string()));
                replaced = true;
            } else {
                kv_pairs.push((k.to_string(), v.to_string()));
            }
        } else {
            kv_pairs.push((pair.to_string(), String::new()));
        }
    }

    if !replaced {
        kv_pairs.push((key.to_string(), value.to_string()));
    }

    if kv_pairs.is_empty() {
        return base.to_string();
    }

    let query_string = kv_pairs
        .into_iter()
        .map(|(k, v)| if v.is_empty() { k } else { format!("{k}={v}") })
        .collect::<Vec<_>>()
        .join("&");

    format!("{base}?{query_string}")
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn http_get(url: &str) -> Result<String, String> {
    let options = utils::RequestOptions {
        method: utils::HttpMethod::Get,
        headers: Some(vec![(
            "User-Agent".to_string(),
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0.0.0 Safari/537.36"
                .to_string(),
        )]),
        body: None,
        timeout_seconds: Some(30),
    };
    let resp = utils::http_request(url, &options).map_err(|e| format!("HTTP GET failed: {e}"))?;

    if resp.status < 200 || resp.status >= 300 {
        return Err(format!("HTTP GET {url} returned status {}", resp.status));
    }

    String::from_utf8(resp.body).map_err(|e| format!("Invalid UTF-8 in response: {e}"))
}

fn fetch_player_js_url() -> Result<String, String> {
    // Check cache first.
    if let Some(cached) = utils::storage_get(STORAGE_KEY_PLAYER_JS) {
        if !cached.is_empty() {
            return Ok(cached);
        }
    }

    // Try www.youtube.com watch page first — this is what ytexplode uses for TV client.
    // Uses a known stable video so the player JS URL is always available.
    if let Ok(html) = http_get(
        "https://www.youtube.com/watch?v=dQw4w9WgXcQ&bpctr=9999999999&has_verified=1&hl=en",
    ) {
        // Pattern from ytexplode: <script src="...player_ias...">
        if let Some(pos) = html.find("player_ias") {
            // Scan backward for src="
            let before = &html[..pos];
            if let Some(src_pos) = before.rfind("src=\"") {
                let after_src = &html[src_pos + 5..];
                if let Some(end) = after_src.find('"') {
                    let rel = &after_src[..end];
                    let url = if rel.starts_with("//") {
                        format!("https:{rel}")
                    } else if rel.starts_with('/') {
                        format!("https://www.youtube.com{rel}")
                    } else {
                        rel.to_string()
                    };
                    let _ = utils::storage_set(STORAGE_KEY_PLAYER_JS, &url);
                    return Ok(url);
                }
            }
        }
        // Also try "jsUrl" pattern in www.youtube.com page
        if let Some(url) = find_between(&html, "\"jsUrl\":\"", "\"") {
            let full = if url.starts_with('/') {
                format!("https://www.youtube.com{url}")
            } else {
                url.to_string()
            };
            let _ = utils::storage_set(STORAGE_KEY_PLAYER_JS, &full);
            return Ok(full);
        }
    }

    // Fallback: fetch YouTube home page and extract player JS URL.
    let html = http_get("https://www.youtube.com/")?;

    // Pattern: "jsUrl":"/s/player/HASH/.../base.js"
    if let Some(url) = find_between(&html, "\"jsUrl\":\"", "\"") {
        if url.starts_with('/') {
            return Ok(format!("https://www.youtube.com{url}"));
        }
        return Ok(url.to_string());
    }

    // Fallback: look for src="/s/player/.../base.js"
    if let Some(url) = find_between(&html, "src=\"/s/player/", "\"") {
        return Ok(format!("https://www.youtube.com/s/player/{url}"));
    }

    // Fallback: search for player hash and construct URL.
    if let Some(hash) = find_player_hash(&html) {
        return Ok(format!(
            "https://www.youtube.com/s/player/{hash}/player_ias.vflset/en_US/base.js"
        ));
    }

    Err("Could not extract player JS URL from YouTube pages".to_string())
}

fn extract_sig_timestamp(js: &str) -> Option<String> {
    // Pattern: signatureTimestamp:NNNNN or sts:NNNNN
    for prefix in &["signatureTimestamp:", "sts:"] {
        if let Some(pos) = js.find(prefix) {
            let start = pos + prefix.len();
            let rest = &js[start..];
            let end = rest
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(rest.len());
            if end > 0 {
                return Some(rest[..end].to_string());
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Core cipher extraction
// ---------------------------------------------------------------------------

fn extract_cipher_ops(js: &str) -> Result<Vec<CipherOp>, String> {
    // 1) YoutubeExplode-style extraction from decipher callsite + helper object.
    if let Ok(ops) = extract_cipher_ops_youtubeexplode(js) {
        if !ops.is_empty() {
            return Ok(ops);
        }
    }

    // 2) Modern fQ+uC approach (player_es6 / player_ias 2024+).
    if let Ok(ops) = extract_cipher_ops_fq(js) {
        if !ops.is_empty() {
            return Ok(ops);
        }
    }

    // 3) Final fallback: classic split/join helper parsing.
    extract_cipher_ops_legacy(js)
}

// ---------------------------------------------------------------------------
// YoutubeExplode-equivalent extraction (PlayerSource.cs)
// ---------------------------------------------------------------------------

fn extract_cipher_ops_youtubeexplode(js: &str) -> Result<Vec<CipherOp>, String> {
    let cipher_callsite =
        find_cipher_callsite(js).ok_or_else(|| "Failed to locate cipher callsite".to_string())?;
    let split_var = extract_split_var_name(&cipher_callsite)
        .ok_or_else(|| "Failed to identify split variable in callsite".to_string())?;

    let (container_name, _) = parse_first_container_call(&cipher_callsite, &split_var)
        .ok_or_else(|| "Failed to identify cipher helper container".to_string())?;

    let cipher_definition = find_cipher_container_definition(js, &container_name)
        .ok_or_else(|| "Failed to locate cipher helper definition".to_string())?;

    let method_types = classify_cipher_methods_youtubeexplode(&cipher_definition);
    if method_types.is_empty() {
        return Err("Failed to classify cipher helper methods".to_string());
    }

    let mut ops = Vec::new();
    for statement in cipher_callsite.split(';') {
        if let Some((_, method_name, index)) = parse_container_call(statement, &split_var) {
            if let Some(kind) = method_types.get(method_name) {
                ops.push(CipherOp {
                    kind: kind.clone(),
                    index,
                });
            }
        }
    }

    if ops.is_empty() {
        return Err("No cipher operations were parsed from callsite".to_string());
    }

    Ok(ops)
}

fn find_cipher_callsite(js: &str) -> Option<String> {
    let mut offset = 0usize;
    while let Some(local) = js[offset..].find("=function(") {
        let func_pos = offset + local;
        let body_open = js[func_pos..].find('{').map(|p| func_pos + p)?;
        let body = find_matching_brace(js, body_open)?;

        if let Some(var_name) = extract_split_var_name(&body) {
            if body.contains(".split(\"\")") || body.contains(".split('')") {
                let return_join1 = format!("return {}.join(\"\")", var_name);
                let return_join2 = format!("return {}.join('')", var_name);
                if body.contains(&return_join1) || body.contains(&return_join2) {
                    return Some(body);
                }
            }
        }

        offset = body_open + 1;
    }
    None
}

fn extract_split_var_name(callsite_or_body: &str) -> Option<String> {
    let split_pos = callsite_or_body
        .find(".split(\"\")")
        .or_else(|| callsite_or_body.find(".split('')"))?;
    let lhs = &callsite_or_body[..split_pos];
    let eq_pos = lhs.rfind('=')?;
    let candidate = lhs[..eq_pos]
        .trim()
        .rsplit(|c: char| !is_ident_char(c))
        .next()
        .unwrap_or("")
        .trim();
    if candidate.is_empty() {
        None
    } else {
        Some(candidate.to_string())
    }
}

fn parse_first_container_call(callsite: &str, split_var: &str) -> Option<(String, String)> {
    for statement in callsite.split(';') {
        if let Some((container, method, _)) = parse_container_call(statement, split_var) {
            return Some((container.to_string(), method.to_string()));
        }
    }
    None
}

fn parse_container_call<'a>(
    statement: &'a str,
    split_var: &str,
) -> Option<(&'a str, &'a str, usize)> {
    let statement = statement.trim();
    let dot_pos = statement.find('.')?;
    let container = statement[..dot_pos].trim();
    if container.is_empty() {
        return None;
    }

    let after_dot = &statement[dot_pos + 1..];
    let open_paren = after_dot.find('(')?;
    let method = after_dot[..open_paren].trim();
    let args = after_dot[open_paren + 1..].trim_end_matches(')').trim();

    let mut arg_parts = args.split(',').map(|s| s.trim());
    let first = arg_parts.next()?;
    if first != split_var {
        return None;
    }

    let index = arg_parts
        .next()
        .and_then(|n| n.parse::<usize>().ok())
        .unwrap_or(0);

    Some((container, method, index))
}

fn find_cipher_container_definition(js: &str, container_name: &str) -> Option<String> {
    for token in [
        format!("var {}={{", container_name),
        format!("var {} = {{", container_name),
        format!("{}={{", container_name),
        format!("{} = {{", container_name),
    ] {
        if let Some(pos) = js.find(&token) {
            let brace_start = pos + token.len() - 1;
            return find_matching_brace(js, brace_start);
        }
    }
    None
}

fn classify_cipher_methods_youtubeexplode(definition: &str) -> HashMap<String, CipherKind> {
    let mut methods: HashMap<String, CipherKind> = HashMap::new();
    let mut pos = 0usize;

    while let Some(local) = definition[pos..].find(":function(") {
        let fn_pos = pos + local;
        let before = &definition[..fn_pos];
        let method_name = before
            .rsplit(|c: char| c == ',' || c == '{' || c == '\n' || c == ';')
            .next()
            .unwrap_or("")
            .trim();

        let body_open = definition[fn_pos..].find("{").map(|i| fn_pos + i);
        if let Some(body_open) = body_open {
            if let Some(body) = find_matching_brace(definition, body_open) {
                let kind = if body.contains('%') {
                    Some(CipherKind::Swap)
                } else if body.contains("splice") {
                    Some(CipherKind::Splice)
                } else if body.contains("reverse") {
                    Some(CipherKind::Reverse)
                } else {
                    None
                };

                if let (Some(kind), false) = (kind, method_name.is_empty()) {
                    methods.insert(method_name.to_string(), kind);
                }

                pos = body_open + body.len() + 2;
                continue;
            }
        }

        pos = fn_pos + 1;
    }

    methods
}

fn is_ident_char(c: char) -> bool {
    c == '_' || c == '$' || c.is_ascii_alphanumeric()
}

/// Modern extraction: fQ dispatch + uC helper object + h[] string array.
fn extract_cipher_ops_fq(js: &str) -> Result<Vec<CipherOp>, String> {
    // Step 1: Parse the h[] string-constant array.
    let h_arr =
        extract_h_array(js).ok_or_else(|| "Failed to locate h[] array in player JS".to_string())?;
    if h_arr.is_empty() {
        return Err("h[] array is empty".to_string());
    }

    // Step 2: Find splice/reverse indices in h[].
    let splice_idx = h_arr
        .iter()
        .position(|s| s == "splice")
        .ok_or_else(|| "Could not find 'splice' in h[] array".to_string())?;
    let reverse_idx = h_arr
        .iter()
        .position(|s| s == "reverse")
        .ok_or_else(|| "Could not find 'reverse' in h[] array".to_string())?;

    // Step 3: Extract uC object and classify its methods.
    let uc_methods = extract_uc_methods(js, splice_idx, reverse_idx)
        .map_err(|e| format!("uC method extraction failed: {e}"))?;
    if uc_methods.is_empty() {
        return Err("No uC cipher methods found".to_string());
    }

    // Step 4: Find fQ outer cipher call → compute V.
    let (v_outer, cipher_block) = extract_fq_cipher_block(js)
        .map_err(|e| format!("fQ cipher block extraction failed: {e}"))?;

    // Step 5: Parse ops from the D|80 block.
    extract_ops_from_block(&cipher_block, v_outer, &h_arr, &uc_methods)
        .map_err(|e| format!("Cipher op parsing failed: {e}"))
}

// ---------------------------------------------------------------------------
// h[] array extraction
// ---------------------------------------------------------------------------

/// Extract all string values from the `var h=[...]` array in player JS.
fn extract_h_array(js: &str) -> Option<Vec<String>> {
    let bracket_pos = ["var h=[", "let h=[", "const h=[", "var h = ["]
        .iter()
        .find_map(|t| js.find(t).map(|p| p + t.len() - 1))?;
    let array_text = extract_bracketed_text(&js[bracket_pos..], '[', ']')?;
    Some(extract_quoted_strings(&array_text))
}

/// Extract interior text between matching bracket/brace pairs starting at s[0].
fn extract_bracketed_text(s: &str, open: char, close: char) -> Option<String> {
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape_next = false;
    let chars: Vec<char> = s.chars().collect();
    let mut start = None;
    for (i, &c) in chars.iter().enumerate() {
        if escape_next {
            escape_next = false;
            continue;
        }
        if c == '\\' && in_string {
            escape_next = true;
            continue;
        }
        if c == '"' {
            in_string = !in_string;
            continue;
        }
        if in_string {
            continue;
        }
        if c == open {
            if depth == 0 {
                start = Some(i);
            }
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                if let Some(s_idx) = start {
                    return Some(chars[s_idx + 1..i].iter().collect());
                }
            }
        }
    }
    None
}

/// Extract all double-quoted string literal values from a JS text segment.
fn extract_quoted_strings(s: &str) -> Vec<String> {
    let mut result = Vec::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '"' {
            let mut j = i + 1;
            let mut value = String::new();
            let mut escape = false;
            while j < chars.len() {
                let c = chars[j];
                if escape {
                    match c {
                        'n' => value.push('\n'),
                        't' => value.push('\t'),
                        'r' => value.push('\r'),
                        '"' => value.push('"'),
                        '\\' => value.push('\\'),
                        _ => {
                            value.push('\\');
                            value.push(c);
                        }
                    }
                    escape = false;
                } else if c == '\\' {
                    escape = true;
                } else if c == '"' {
                    break;
                } else {
                    value.push(c);
                }
                j += 1;
            }
            result.push(value);
            i = j + 1;
        } else {
            i += 1;
        }
    }
    result
}

// ---------------------------------------------------------------------------
// uC method extraction
// ---------------------------------------------------------------------------

/// Parse `var uC={NAME:function(...){BODY}, ...}` and map each method name to
/// its CipherKind based on which h[N] index is referenced in the method body.
fn extract_uc_methods(
    js: &str,
    splice_idx: usize,
    reverse_idx: usize,
) -> Result<HashMap<String, CipherKind>, String> {
    let token = "var uC={";
    let pos = js
        .find(token)
        .ok_or_else(|| "Could not find 'var uC={' in player JS".to_string())?;
    let brace_start = pos + token.len() - 1;
    let inner = extract_bracketed_text(&js[brace_start..], '{', '}')
        .ok_or_else(|| "Could not find matching '}' for uC object".to_string())?;

    let mut methods: HashMap<String, CipherKind> = HashMap::new();
    let mut search = inner.as_str();
    while let Some(fn_pos) = search.find(":function(") {
        let before = &search[..fn_pos];
        let name_start = before
            .rfind(|c: char| c == ',' || c == '{' || c == '\n' || c == ';')
            .map(|p| p + 1)
            .unwrap_or(0);
        let name = before[name_start..].trim();
        if !name.is_empty() {
            let body_search_start = fn_pos + ":function(".len();
            if let Some(pc) = search[body_search_start..]
                .find("){")
                .map(|p| body_search_start + p + 1)
            {
                let body = extract_bracketed_text(&search[pc..], '{', '}').unwrap_or_default();
                if let Some(k) = classify_uc_body(&body, splice_idx, reverse_idx) {
                    methods.insert(name.to_string(), k);
                }
            }
        }
        search = &search[fn_pos + 1..];
    }
    Ok(methods)
}

/// Classify a uC method body as Splice, Reverse, or Swap.
fn classify_uc_body(body: &str, splice_idx: usize, reverse_idx: usize) -> Option<CipherKind> {
    if body.contains(&format!("h[{splice_idx}]")) {
        Some(CipherKind::Splice)
    } else if body.contains(&format!("h[{reverse_idx}]")) {
        Some(CipherKind::Reverse)
    } else if body.contains("D[0]") {
        Some(CipherKind::Swap)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// fQ cipher block extraction
// ---------------------------------------------------------------------------

/// Find the outer cipher call `fQ(D, X, fQ(inner, inner_x, m.s))`,
/// compute `V = X ^ D`, then locate and return the `if((D|80)==D){...}` block.
fn extract_fq_cipher_block(js: &str) -> Result<(usize, String), String> {
    let cipher_call_start = find_fq_cipher_call(js)
        .ok_or_else(|| "Could not find fQ cipher call pattern in player JS".to_string())?;
    let call_slice = &js[cipher_call_start..];
    let (d_outer, x_outer) = parse_fq_outer_args(call_slice)
        .ok_or_else(|| "Failed to parse fQ outer call arguments".to_string())?;
    let v_outer = x_outer ^ d_outer;

    let fq_fn_pos = js
        .find("fQ=function(D,X,B,C){")
        .or_else(|| js.find("fQ =function(D,X,B,C){"))
        .ok_or_else(|| "Could not find fQ function definition".to_string())?;
    let fq_brace = js[fq_fn_pos..]
        .find('{')
        .map(|p| fq_fn_pos + p)
        .ok_or_else(|| "Could not find opening brace of fQ function".to_string())?;
    let fq_body = extract_bracketed_text(&js[fq_brace..], '{', '}')
        .ok_or_else(|| "Could not extract fQ function body".to_string())?;

    let d80_marker = "if((D|80)==D)";
    let d80_pos = fq_body
        .find(d80_marker)
        .ok_or_else(|| "Could not find if((D|80)==D) block in fQ body".to_string())?;
    let d80_brace_start = fq_body[d80_pos..]
        .find('{')
        .map(|p| d80_pos + p)
        .ok_or_else(|| "No '{' after if((D|80)==D)".to_string())?;
    let cipher_block = extract_bracketed_text(&fq_body[d80_brace_start..], '{', '}')
        .ok_or_else(|| "Could not extract if((D|80)==D) block content".to_string())?;

    Ok((v_outer, cipher_block))
}

/// Scan the JS for `fQ(NUMBER, NUMBER, fQ(NUMBER, NUMBER, m.s))`.
fn find_fq_cipher_call(js: &str) -> Option<usize> {
    let mut search = js;
    let mut base_offset = 0usize;
    while let Some(pos) = search.find("fQ(") {
        if is_fq_cipher_call(&search[pos..]) {
            return Some(base_offset + pos);
        }
        base_offset += pos + 3;
        search = &search[pos + 3..];
    }
    None
}

fn is_fq_cipher_call(slice: &str) -> bool {
    let after_fq = &slice[3..];
    if let Some((_, rest1)) = parse_number(after_fq) {
        if rest1.starts_with(',') {
            if let Some((_, rest3)) = parse_number(&rest1[1..]) {
                if rest3.starts_with(",fQ(") {
                    if let Some((_, rest5)) = parse_number(&rest3[4..]) {
                        if rest5.starts_with(',') {
                            if let Some((_, rest7)) = parse_number(&rest5[1..]) {
                                return rest7.starts_with(",m.s)");
                            }
                        }
                    }
                }
            }
        }
    }
    false
}

fn parse_fq_outer_args(slice: &str) -> Option<(usize, usize)> {
    let after_fq = slice.strip_prefix("fQ(")?;
    let (d, rest1) = parse_number(after_fq)?;
    let (x, _) = parse_number(rest1.strip_prefix(',')?)?;
    Some((d, x))
}

// ---------------------------------------------------------------------------
// Op parsing from cipher block
// ---------------------------------------------------------------------------

/// Parse `uC[h[V^CONST]](x, [V^]? ARG)` calls from the cipher block body.
fn extract_ops_from_block(
    block: &str,
    v: usize,
    h_arr: &[String],
    uc_methods: &HashMap<String, CipherKind>,
) -> Result<Vec<CipherOp>, String> {
    let mut ops = Vec::new();
    let mut search = block;
    let marker = "uC[h[V^";
    while let Some(pos) = search.find(marker) {
        let after = &search[pos + marker.len()..];
        let (method_xor_const, rest1) = match parse_number(after) {
            Some(x) => x,
            None => {
                search = &search[pos + marker.len()..];
                continue;
            }
        };
        // Expect "]](" next
        let rest2 = match rest1.strip_prefix("]](x") {
            Some(r) => r,
            None => {
                search = &search[pos + marker.len()..];
                continue;
            }
        };
        let actual_arg = if let Some(vx_rest) = rest2.strip_prefix(",V^") {
            parse_number(vx_rest).map(|(xc, _)| v ^ xc).unwrap_or(0)
        } else if let Some(lit_rest) = rest2.strip_prefix(',') {
            parse_number(lit_rest).map(|(n, _)| n).unwrap_or(0)
        } else {
            0
        };
        let method_h_idx = v ^ method_xor_const;
        let method_name = h_arr.get(method_h_idx).map(|s| s.as_str()).unwrap_or("");
        if let Some(kind) = uc_methods.get(method_name) {
            ops.push(CipherOp {
                kind: kind.clone(),
                index: actual_arg,
            });
        }
        search = &search[pos + marker.len()..];
    }
    if ops.is_empty() {
        Err("No cipher operations found in fQ dispatch block".to_string())
    } else {
        Ok(ops)
    }
}

// ---------------------------------------------------------------------------
// Number parsing utility
// ---------------------------------------------------------------------------

/// Parse a decimal integer from the start of `s`.
fn parse_number(s: &str) -> Option<(usize, &str)> {
    let end = s
        .char_indices()
        .take_while(|(_, c)| c.is_ascii_digit())
        .last()
        .map(|(i, c)| i + c.len_utf8())?;
    if end == 0 {
        return None;
    }
    let n: usize = s[..end].parse().ok()?;
    Some((n, &s[end..]))
}

// ---------------------------------------------------------------------------
// Legacy cipher extraction (pre-2024 player JS)
// ---------------------------------------------------------------------------

fn extract_cipher_ops_legacy(js: &str) -> Result<Vec<CipherOp>, String> {
    let fn_body = find_decipher_function_body_legacy(js)
        .ok_or("Could not find decipher function in player JS")?;
    let helper_name = find_helper_object_name_legacy(&fn_body)
        .ok_or("Could not find helper object name in decipher function")?;
    let method_types = classify_helper_methods_legacy(js, &helper_name)?;
    parse_cipher_calls_legacy(&fn_body, &method_types)
}

fn find_decipher_function_body_legacy(js: &str) -> Option<String> {
    let split_marker = "a=a.split(\"\");";
    let join_marker = "return a.join(\"\")";
    let split_pos = js.find(split_marker)?;
    let after_split = &js[split_pos..];
    let join_offset = after_split.find(join_marker)?;
    let body_start = split_pos + split_marker.len();
    let body_end = split_pos + join_offset;
    let body = js[body_start..body_end].trim().trim_end_matches(';');
    Some(body.to_string())
}

fn find_helper_object_name_legacy(fn_body: &str) -> Option<String> {
    let dot_pos = fn_body.find('.')?;
    let name = fn_body[..dot_pos].trim().trim_start_matches(';');
    if name.is_empty() {
        return None;
    }
    Some(name.to_string())
}

fn classify_helper_methods_legacy(
    js: &str,
    helper_name: &str,
) -> Result<Vec<(String, CipherKind)>, String> {
    let mut obj_start = None;
    for pattern in &[
        format!("var {}={{", helper_name),
        format!("{} = {{", helper_name),
        format!("{}={{", helper_name),
    ] {
        if let Some(pos) = js.find(pattern.as_str()) {
            obj_start = Some(pos + pattern.len() - 1);
            break;
        }
    }
    let obj_start =
        obj_start.ok_or_else(|| format!("Could not find helper object '{helper_name}'"))?;
    let obj_body =
        find_matching_brace(js, obj_start).ok_or("Could not find end of helper object")?;
    let mut methods = Vec::new();
    let mut pos = 0;
    while pos < obj_body.len() {
        if let Some(colon_pos) = obj_body[pos..].find(":function(") {
            let name_start = obj_body[..pos + colon_pos]
                .rfind(|c: char| c == ',' || c == '{' || c == '\n')
                .map(|p| p + 1)
                .unwrap_or(pos);
            let method_name = obj_body[name_start..pos + colon_pos].trim().to_string();
            let fn_start_search = pos + colon_pos + ":function(".len();
            if let Some(paren_close) = obj_body[fn_start_search..].find("){") {
                let body_start = fn_start_search + paren_close + 1;
                if let Some(body) = find_matching_brace(&obj_body, body_start) {
                    if let Some(kind) = classify_method_body_legacy(&body) {
                        methods.push((method_name, kind));
                    }
                    pos = body_start + body.len() + 2;
                    continue;
                }
            }
        }
        pos += 1;
    }
    if methods.is_empty() {
        return Err("No cipher methods found in helper object".to_string());
    }
    Ok(methods)
}

fn classify_method_body_legacy(body: &str) -> Option<CipherKind> {
    if body.contains("reverse") {
        Some(CipherKind::Reverse)
    } else if body.contains("splice") {
        Some(CipherKind::Splice)
    } else if body.contains("var c=") || (body.contains("a[0]") && body.contains("a[b")) {
        Some(CipherKind::Swap)
    } else {
        None
    }
}

fn parse_cipher_calls_legacy(
    fn_body: &str,
    method_types: &[(String, CipherKind)],
) -> Result<Vec<CipherOp>, String> {
    let mut ops = Vec::new();
    for call in fn_body.split(';') {
        let call = call.trim();
        if call.is_empty() {
            continue;
        }
        if let Some(dot_pos) = call.find('.') {
            let after_dot = &call[dot_pos + 1..];
            if let Some(paren_pos) = after_dot.find('(') {
                let method_name = &after_dot[..paren_pos];
                let args_str = &after_dot[paren_pos + 1..].trim_end_matches(')');
                if let Some((_, kind)) = method_types.iter().find(|(n, _)| n == method_name) {
                    let index = args_str
                        .split(',')
                        .nth(1)
                        .and_then(|s| s.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    ops.push(CipherOp {
                        kind: kind.clone(),
                        index,
                    });
                }
            }
        }
    }
    if ops.is_empty() {
        return Err("No cipher operations parsed from function body".to_string());
    }
    Ok(ops)
}

fn find_matching_brace(s: &str, start: usize) -> Option<String> {
    if s.as_bytes().get(start) != Some(&b'{') {
        return None;
    }
    let mut depth = 0;
    for (i, c) in s[start..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(s[start + 1..start + i].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

fn find_between<'a>(haystack: &'a str, start_marker: &str, end_marker: &str) -> Option<&'a str> {
    let start = haystack.find(start_marker)?;
    let after = start + start_marker.len();
    let end = haystack[after..].find(end_marker)?;
    Some(&haystack[after..after + end])
}

fn find_player_hash(html: &str) -> Option<String> {
    // Look for any /s/player/HASH/ pattern
    let marker = "/s/player/";
    let pos = html.find(marker)?;
    let after = pos + marker.len();
    let rest = &html[after..];
    let end = rest.find('/')?;
    let hash = &rest[..end];
    if hash.len() >= 8 && hash.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(hash.to_string())
    } else {
        None
    }
}

/// Parse a URL query string into key-value pairs.
pub fn parse_query_string(qs: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    for pair in qs.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            map.insert(
                urlencoding::decode(k).unwrap_or_default().into_owned(),
                urlencoding::decode(v).unwrap_or_default().into_owned(),
            );
        }
    }
    map
}

// ---------------------------------------------------------------------------
// Serialization for caching ops to storage
// ---------------------------------------------------------------------------

fn serialize_ops(ops: &[CipherOp]) -> String {
    // Simple format: "R0,S3,W5" for Reverse(0), Splice(3), Swap(5)
    ops.iter()
        .map(|op| {
            let prefix = match op.kind {
                CipherKind::Reverse => "R",
                CipherKind::Splice => "S",
                CipherKind::Swap => "W",
            };
            format!("{}{}", prefix, op.index)
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn parse_cached_ops(ops_json: &str, sig_ts: &str) -> Result<CipherManifest, String> {
    if ops_json.is_empty() || sig_ts.is_empty() {
        return Err("Empty cached data".to_string());
    }

    let ops: Result<Vec<CipherOp>, String> = ops_json
        .split(',')
        .map(|s| {
            let s = s.trim();
            if s.len() < 2 {
                return Err("Invalid op format".to_string());
            }
            let kind = match &s[..1] {
                "R" => CipherKind::Reverse,
                "S" => CipherKind::Splice,
                "W" => CipherKind::Swap,
                _ => return Err(format!("Unknown op kind: {}", &s[..1])),
            };
            let index = s[1..]
                .parse::<usize>()
                .map_err(|_| format!("Invalid op index: {}", &s[1..]))?;
            Ok(CipherOp { kind, index })
        })
        .collect();

    Ok(CipherManifest {
        ops: ops?,
        sig_timestamp: sig_ts.to_string(),
    })
}
