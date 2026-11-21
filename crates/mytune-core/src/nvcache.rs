//! NVIDIA App's lowest in-game settings for a game, read from its local cache
//! (`%LOCALAPPDATA%\NVIDIA Corporation\NVIDIA App\NvBackend\Recommendations\<game>`): `pops.pub.tsv` (JSON: the
//! setting names and NVIDIA's presets, the first one the lowest) and `wrappers\current_game.lua` (compiled
//! Lua 5.1), which says where each setting lives in the game's own file and what each label is written as.
//!
//! NVIDIA's Lua is never run. The main chunk of the wrapper is straight-line code that builds tables and calls
//! `add_property{...}`; `calls` follows the registers through it and rebuilds those tables. A wrapper with a
//! jump, a loop or anything else in its main chunk gives nothing. The cache is untrusted: sizes are bounded,
//! every read is checked, and a setting is kept only when all of it is plain (see `lowest`).
//!
//! shortcut: only wrappers of the `context_v1` kind with an ini or JSON file are understood. Older wrappers
//! (a `Setting` table plus Lua functions that find and write the file), registry sources and custom parsers
//! give no settings; they need a per-game look before more is taken from them.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// No file of the cache is read past this size.
const MAX: u64 = 2 * 1024 * 1024;

// ---------------------------------------------------------------- Lua 5.1 bytecode

struct Rd<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Rd<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.b.get(self.at..self.at.checked_add(n)?)?;
        self.at += n;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|s| s[0])
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn size(&mut self) -> Option<usize> {
        usize::try_from(u64::from_le_bytes(self.take(8)?.try_into().ok()?)).ok()
    }
    /// A Lua string: its size counts the closing zero byte; size 0 is "no string".
    fn string(&mut self) -> Option<String> {
        let n = self.size()?;
        let s = self.take(n)?;
        Some(String::from_utf8_lossy(&s[..n.saturating_sub(1)]).into_owned())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Konst {
    Nil,
    Bool(bool),
    Num(f64),
    Str(String),
}

#[derive(Debug)]
pub struct Func {
    /// Upvalues: a `CLOSURE` of this function is followed by that many pseudo-instructions.
    pub nups: u8,
    pub params: u8,
    pub code: Vec<u32>,
    pub consts: Vec<Konst>,
    pub protos: Vec<Func>,
}

/// Lua 5.1, official format, little endian, 4-byte int, 8-byte size_t, 4-byte instruction, 8-byte double.
const HEADER: [u8; 12] = [0x1B, b'L', b'u', b'a', 0x51, 0, 1, 4, 8, 4, 8, 0];

/// The compiled chunk's main function; `None`: not such a file, or cut off.
pub fn parse(bytes: &[u8]) -> Option<Func> {
    if !bytes.starts_with(&HEADER) {
        return None;
    }
    func(&mut Rd { b: bytes, at: HEADER.len() }, 0)
}

fn func(r: &mut Rd, depth: usize) -> Option<Func> {
    if depth > 16 {
        return None;
    }
    r.string()?; // source name
    r.take(8)?; // first and last line
    let (nups, params) = (r.u8()?, r.u8()?);
    r.take(2)?; // vararg flag, stack size
    // every count below is checked against the bytes left before anything is stored
    let n = r.u32()? as usize;
    let code = r.take(n.checked_mul(4)?)?.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
    let mut consts = Vec::new();
    for _ in 0..r.u32()? {
        consts.push(match r.u8()? {
            0 => Konst::Nil,
            1 => Konst::Bool(r.u8()? != 0),
            3 => Konst::Num(f64::from_le_bytes(r.take(8)?.try_into().ok()?)),
            4 => Konst::Str(r.string()?),
            // NVIDIA's own type: a UTF-16 string, its size in characters with the closing zero
            9 => {
                let n = r.size()?;
                let wide: Vec<u16> = r.take(n.checked_mul(2)?)?.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
                Konst::Str(String::from_utf16_lossy(&wide).trim_end_matches('\0').to_string())
            }
            _ => return None,
        });
    }
    let mut protos = Vec::new();
    for _ in 0..r.u32()? {
        protos.push(func(r, depth + 1)?);
    }
    let n = r.u32()? as usize;
    r.take(n.checked_mul(4)?)?; // line of each instruction
    for _ in 0..r.u32()? {
        r.string()?; // local variable
        r.take(8)?;
    }
    for _ in 0..r.u32()? {
        r.string()?; // upvalue name
    }
    Some(Func { nups, params, code, consts, protos })
}

// ---------------------------------------------------------------- what the main chunk builds

/// A value in a register, as far as it can be known without running anything.
#[derive(Debug, Clone)]
enum V {
    Nil,
    Bool(bool),
    Num(f64),
    Str(String),
    /// (list part, `key = value` part, in the order written)
    Table(Vec<Rc<V>>, Vec<(Rc<V>, Rc<V>)>),
    Global(String),
    /// `object:name`
    Method(Rc<V>, String),
    /// (function, arguments); the first argument of a method call is the object
    Call(Rc<V>, Vec<Rc<V>>),
    /// A function of the chunk, by its index
    Closure(usize),
    Unknown,
}

fn konst(f: &Func, i: usize) -> Option<Rc<V>> {
    Some(Rc::new(match f.consts.get(i)? {
        Konst::Nil => V::Nil,
        Konst::Bool(b) => V::Bool(*b),
        Konst::Num(n) => V::Num(*n),
        Konst::Str(s) => V::Str(s.clone()),
    }))
}

/// Every call the main chunk makes, in order. `None`: the chunk is not straight-line table building.
/// shortcut: a table held in two registers is copied when one of them changes it (Lua shares it); the
/// wrappers fill a table before they hand it on.
fn calls(f: &Func) -> Option<Vec<Rc<V>>> {
    let unknown = Rc::new(V::Unknown);
    let mut reg: Vec<Rc<V>> = vec![unknown.clone(); 512];
    let mut globals: HashMap<String, Rc<V>> = HashMap::new();
    let (mut out, mut top, mut pc) = (Vec::new(), 0usize, 0usize);
    // copies of shared tables are paid from this, so a crafted file cannot make the work explode
    let mut budget = 1_000_000usize;
    while let Some(&i) = f.code.get(pc) {
        pc += 1;
        let (op, a, c, b, bx) = (i & 63, (i >> 6 & 255) as usize, (i >> 14 & 511) as usize, (i >> 23 & 511) as usize, (i >> 14) as usize);
        let rk = |reg: &[Rc<V>], x: usize| if x >= 256 { konst(f, x - 256) } else { reg.get(x).cloned() };
        let name = |v: Option<Rc<V>>| match v.as_deref() {
            Some(V::Str(s)) => Some(s.clone()),
            _ => None,
        };
        let new = match op {
            0 => reg.get(b)?.clone(),                                        // MOVE
            1 => konst(f, bx)?,                                              // LOADK
            2 if c == 0 => Rc::new(V::Bool(b != 0)),                         // LOADBOOL
            3 => {
                for r in a..=b {
                    *reg.get_mut(r)? = Rc::new(V::Nil);                      // LOADNIL
                }
                continue;
            }
            4 | 6 | 21 => unknown.clone(),                                   // GETUPVAL, GETTABLE, CONCAT
            5 => {
                let n = name(konst(f, bx))?;                                 // GETGLOBAL
                globals.get(&n).cloned().unwrap_or_else(|| Rc::new(V::Global(n)))
            }
            7 => {
                globals.insert(name(konst(f, bx))?, reg.get(a)?.clone());    // SETGLOBAL
                continue;
            }
            9 => {
                let (key, val) = (rk(&reg, b)?, rk(&reg, c)?);               // SETTABLE
                let t = reg.get_mut(a)?;
                if let V::Table(list, map) = &**t {
                    if Rc::strong_count(t) > 1 {
                        budget = budget.checked_sub(list.len() + map.len() + 1)?;
                    }
                    if let V::Table(_, map) = Rc::make_mut(t) {
                        map.push((key, val));
                    }
                }
                continue;
            }
            10 => Rc::new(V::Table(Vec::new(), Vec::new())),                 // NEWTABLE
            11 => {
                let object = reg.get(b)?.clone();                            // SELF
                let method = name(rk(&reg, c))?;
                *reg.get_mut(a + 1)? = object.clone();
                Rc::new(V::Method(object, method))
            }
            28 => {
                let n = if b == 0 { top.checked_sub(a + 1)? } else { b - 1 }; // CALL
                budget = budget.checked_sub(n + 1)?;
                let call = Rc::new(V::Call(reg.get(a)?.clone(), reg.get(a + 1..a + 1 + n)?.to_vec()));
                out.push(call.clone());
                if c == 0 {
                    top = a + 1; // "every result": kept as one value
                }
                call
            }
            34 if c != 0 => {
                let n = if b == 0 { top.checked_sub(a + 1)? } else { b };    // SETLIST
                budget = budget.checked_sub(n + 1)?;
                let items = reg.get(a + 1..a + 1 + n)?.to_vec();
                let t = reg.get_mut(a)?;
                if let V::Table(list, map) = &**t {
                    if Rc::strong_count(t) > 1 {
                        budget = budget.checked_sub(list.len() + map.len() + 1)?;
                    }
                }
                match Rc::make_mut(t) {
                    V::Table(list, _) => list.extend(items),
                    _ => return None,
                }
                continue;
            }
            36 => {
                pc += f.protos.get(bx)?.nups as usize;                       // CLOSURE and its upvalue list
                Rc::new(V::Closure(bx))
            }
            30 => break,                                                     // RETURN
            _ => return None,
        };
        *reg.get_mut(a)? = new;
    }
    Some(out)
}

// ---------------------------------------------------------------- the lowest preset

/// One setting of NVIDIA's lowest preset, with where and how the game's file holds it.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    /// The game's file as the wrapper names it: starts with `%profile%`, `%localappdata%` or `%appdata%`,
    /// may have one `*` in the file name.
    pub file: String,
    /// An ini file (`[section]`, `key=value`); else one flat JSON object.
    pub ini: bool,
    pub section: String,
    pub key: String,
    /// NVIDIA's name of the setting.
    pub label: String,
    /// (label, value as written in the file); empty for a number.
    pub choices: Vec<(String, String)>,
    /// A number: (min, max, digits after the point).
    pub range: Option<(f64, f64, usize)>,
    /// The lowest preset's value, as written in the file.
    pub best: String,
    /// JSON only: the values are strings (in quotes); else `true`/`false` or numbers.
    pub text: bool,
}

fn field<'a>(map: &'a [(Rc<V>, Rc<V>)], key: &str) -> Option<&'a V> {
    let mut hits = map.iter().filter(|(k, _)| matches!(&**k, V::Str(s) if s == key));
    let first = hits.next()?;
    hits.next().is_none().then_some(&*first.1) // written twice: not plain
}

/// `require('<module>')`
fn module(v: &V) -> Option<&str> {
    match v {
        V::Call(f, args) if matches!(&**f, V::Global(g) if g == "require") => match args.as_slice() {
            [m] => match &**m {
                V::Str(s) => Some(s),
                _ => None,
            },
            _ => None,
        },
        _ => None,
    }
}

/// The file behind `<parser>:new(...)`: (path, is ini).
fn parser_file(v: &V) -> Option<(&str, bool)> {
    let V::Call(f, args) = v else { return None };
    let V::Method(object, method) = &**f else { return None };
    if method != "new" {
        return None;
    }
    let rest = args.get(1..)?;
    let paths: Vec<&str> = rest.iter().filter_map(|a| if let V::Str(s) = &**a { Some(s.as_str()) } else { None }).collect();
    let [path] = paths.as_slice() else { return None };
    let kind = |m: &str| match m {
        "ini_parser_v1" => Some(true),
        "json_parser_v1" => Some(false),
        _ => None,
    };
    let ini = match module(object)? {
        // the newest file that fits a pattern, read by the parser handed in
        "glob_parser_v1" => {
            let inner: Vec<bool> = rest.iter().filter_map(|a| module(a)).filter_map(kind).collect();
            match inner.as_slice() {
                [one] => *one,
                _ => return None,
            }
        }
        m => {
            if rest.len() != 1 || path.contains('*') {
                return None;
            }
            kind(m)?
        }
    };
    Some((path, ini))
}

/// A number as a JSON or ini file has it: no `.0` on whole numbers.
fn number(n: f64) -> Option<String> {
    if !n.is_finite() {
        None
    } else if n.fract() == 0.0 && n.abs() < 1e15 {
        Some(format!("{}", n as i64))
    } else {
        Some(format!("{n}"))
    }
}

/// A value that can sit in a line, an attribute or between quotes without breaking them.
fn plain(v: &str) -> bool {
    !v.is_empty() && v.trim() == v && !v.contains(['\r', '\n', '"', '<', '>', '\\', '\0'])
}

/// Names and keys that are never written: resolution, window mode, render scale, upscalers, FPS caps and binds.
fn denied(name: &str, key: &str) -> bool {
    let (name, key) = (name.to_lowercase(), key.to_lowercase());
    name == "resolution"
        || ["display mode", "render", "scale", "upscal", "dlss", "fsr", "super resolution", "frame gen", "window", "fps", "frame rate", "framerate", "frame limit", "frame cap", "bind", "hotkey", "keyboard", "mouse", "sensitivity"].iter().any(|w| name.contains(w))
        || ["resolution", "fullscreen", "screenpercentage", "renderscale", "render_scale", "window", "fps", "framerate", "frame_rate", "framelimit", "frame_limit", "framecap", "frame_cap", "maxframe", "max_frame", "bind", "hotkey", "keyboard", "key_", "mouse", "sensitivity"].iter().any(|w| key.contains(w))
}

/// `function(v) return v end`
fn identity(f: &Func, v: Option<&V>) -> bool {
    const RETURN_FIRST: u32 = 30 | 2 << 23;
    match v {
        None => true,
        Some(V::Closure(i)) => f.protos.get(*i).is_some_and(|p| p.params >= 1 && p.code.len() <= 2 && p.code.first() == Some(&RETURN_FIRST)),
        _ => false,
    }
}

fn one(main: &Func, t: &[(Rc<V>, Rc<V>)], name: &str, low: &str) -> Result<Found, &'static str> {
    let Some(V::Table(sources, extra)) = field(t, "sources") else { return Err("no sources") };
    let [source] = sources.as_slice() else { return Err("several sources") };
    if !extra.is_empty() {
        return Err("sources not plain");
    }
    // `<parser>:ref(key)`; a third argument is a READ or WRITE flag
    let V::Call(f, args) = &**source else { return Err("source not plain") };
    let (V::Method(parser, method), [_, key]) = (&**f, args.as_slice()) else { return Err("flagged source") };
    let V::Str(key) = &**key else { return Err("source not plain") };
    if method != "ref" {
        return Err("source not plain");
    }
    let Some((file, ini)) = parser_file(parser) else { return Err("not an ini or JSON file") };
    let (section, key) = match ini {
        true => key.split_once(';').ok_or("no section")?,
        false => ("", key.as_str()),
    };
    if !plain(key) || (ini && !plain(section)) || key.contains([';', '=', ':', '[', ']']) || section.contains(['[', ']']) {
        return Err("odd key");
    }
    if denied(name, key) {
        return Err("resolution, window or scaling");
    }
    let mut found = Found { file: file.into(), ini, section: section.into(), key: key.into(), label: name.into(), choices: vec![], range: None, best: String::new(), text: true };
    match field(t, "type") {
        Some(V::Str(ty)) if ty == "ENUM" => {
            let Some(V::Table(rows, _)) = field(t, "values") else { return Err("values made by a function") };
            let mut kinds = [false; 3];
            for row in rows {
                let V::Table(row, _) = &**row else { return Err("values not plain") };
                let [label, value] = row.as_slice() else { return Err("values not plain") };
                let V::Str(label) = &**label else { return Err("values not plain") };
                let (kind, value) = match &**value {
                    V::Str(s) => (0, s.clone()),
                    V::Bool(b) => (1, b.to_string()),
                    V::Num(n) => (2, number(*n).ok_or("values not plain")?),
                    V::Global(_) => continue, // NULL, ANY: how a missing key reads, nothing to write
                    _ => return Err("values not plain"),
                };
                if !plain(&value) {
                    return Err("odd value");
                }
                if found.choices.iter().any(|(l, v)| l == label && *v != value) {
                    return Err("a label with two values");
                }
                kinds[kind] = true;
                found.choices.push((label.clone(), value));
            }
            if kinds.iter().filter(|k| **k).count() != 1 || (ini && !kinds[0]) {
                return Err("mixed value types");
            }
            found.text = kinds[0];
            found.best = found.choices.iter().find(|(l, _)| l == low).ok_or("lowest label not in the list")?.1.clone();
        }
        // shortcut: numbers only in JSON files; an ini's number format is the game's own
        Some(V::Str(ty)) if ty == "FLOAT" && !ini => {
            let Some(V::Table(_, range)) = field(t, "range") else { return Err("no range") };
            let num = |k| match field(range, k) {
                Some(V::Num(n)) if n.is_finite() => Some(*n),
                _ => None,
            };
            let (Some(min), Some(max), Some(digits)) = (num("min"), num("max"), num("precision")) else { return Err("no range") };
            if !identity(main, field(range, "decode")) || !identity(main, field(range, "encode")) || !(0.0..=6.0).contains(&digits) || min > max {
                return Err("number converted by a function");
            }
            let value: f64 = low.parse().ok().filter(|v: &f64| (min..=max).contains(v)).ok_or("lowest value outside the range")?;
            found.range = Some((min, max, digits as usize));
            found.best = format!("{value:.*}", digits as usize);
            found.text = false;
        }
        _ => return Err("type not handled"),
    }
    Ok(found)
}

/// The lowest preset's settings that are plain enough to write, and why the others were left out.
/// Kept: one source without a flag, in an ini or JSON file; a fixed list of `{label, value}` (or a number with
/// no conversion); the preset's label in that list. Never: resolution, window mode, render scale, upscalers.
pub fn lowest(wrapper: &[u8], pops: &[u8]) -> (Vec<Found>, Vec<String>) {
    let mut skipped = Vec::new();
    let Ok(pops) = serde_json::from_slice::<serde_json::Value>(pops) else { return (vec![], vec!["presets unreadable".into()]) };
    // the settings in order; a preset's values are numbered from 1 into this list
    let names: Vec<&str> = pops["settings"].as_array().into_iter().flatten().filter_map(|s| Some(s.as_object()?.keys().next()?.as_str())).collect();
    let values = &pops["pops"][0]["values"];
    let low: HashMap<&str, &str> = names.iter().enumerate().filter_map(|(i, n)| Some((*n, values[(i + 1).to_string()].as_str()?))).collect();
    if names.len() != pops["settings"].as_array().map_or(0, Vec::len) || low.is_empty() {
        return (vec![], vec!["presets unreadable".into()]);
    }
    let Some(main) = parse(wrapper) else { return (vec![], vec!["wrapper unreadable".into()]) };
    let Some(calls) = calls(&main) else { return (vec![], vec!["wrapper is not plain table code".into()]) };
    let mut out: Vec<Found> = Vec::new();
    let mut twice = Vec::new();
    for call in &calls {
        let V::Call(f, args) = &**call else { continue };
        let (V::Method(_, method), Some(V::Table(_, t))) = (&**f, args.get(1).map(|a| &**a)) else { continue };
        let Some(V::Str(name)) = field(t, "name") else { continue };
        let Some(low) = low.get(name.as_str()).filter(|_| method == "add_property") else { continue };
        match one(&main, t, name, low) {
            Ok(found) => {
                if out.iter().any(|o| o.label == found.label || (o.file == found.file && o.section.eq_ignore_ascii_case(&found.section) && o.key.eq_ignore_ascii_case(&found.key))) {
                    twice.push((found.label.clone(), found.file.clone(), found.section.to_lowercase(), found.key.to_lowercase()));
                }
                out.push(found);
            }
            Err(why) => skipped.push(format!("{name} ({why})")),
        }
    }
    // a name or a key declared twice: neither is taken
    for (label, file, section, key) in twice {
        out.retain(|o| o.label != label && !(o.file == file && o.section.to_lowercase() == section && o.key.to_lowercase() == key));
        skipped.push(format!("{label} (declared twice)"));
    }
    if out.is_empty() && skipped.is_empty() {
        skipped.push("no add_property calls (older wrapper kind)".into());
    }
    (out, skipped)
}

fn read(path: &Path) -> Option<Vec<u8>> {
    fs::metadata(path).ok().filter(|m| m.is_file() && m.len() <= MAX)?;
    fs::read(path).ok()
}

/// A folder name from the cache's metadata: a hash, nothing that leaves the game's folder.
fn hash_dir(folder: &Path, file: &str, key: &str) -> Option<PathBuf> {
    let meta: serde_json::Value = serde_json::from_slice(&read(&folder.join(file))?).ok()?;
    let name = meta[key].as_str().filter(|n| !n.is_empty() && n.len() <= 128 && n.bytes().all(|b| b.is_ascii_hexdigit()))?;
    Some(folder.join(name))
}

/// The lowest preset of the game with this title (`gamelib::title_key`) from the cache in `root`, and what
/// was left out. No cache, no such game or anything unreadable: nothing.
pub fn for_game(root: &Path, title_key: &str) -> (Vec<Found>, Vec<String>) {
    let slugs: Vec<PathBuf> = fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|d| crate::gamelib::title_key(&d.file_name().to_string_lossy().replace('_', " ")) == title_key)
        .map(|d| d.path())
        .filter(|p| p.is_dir())
        .collect();
    let [folder] = slugs.as_slice() else { return (vec![], vec![]) };
    let wrapper = hash_dir(folder, "commonfiles_metadata.json", "c").and_then(|d| read(&d.join("wrappers").join("current_game.lua")));
    // the presets sit one folder down (`regular`, `regular_rtx`): only one may be there
    let pops = hash_dir(folder, "profiles_metadata.json", "ce").and_then(|d| {
        let found: Vec<PathBuf> = fs::read_dir(d).into_iter().flatten().flatten().map(|f| f.path().join("pops.pub.tsv")).filter(|p| p.is_file()).collect();
        match found.as_slice() {
            [one] => read(one),
            _ => None,
        }
    });
    match (wrapper, pops) {
        (Some(w), Some(p)) => lowest(&w, &p),
        _ => (vec![], vec!["cache files missing".into()]),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A tiny assembler for test chunks.
    #[derive(Default)]
    pub(crate) struct Asm {
        code: Vec<u32>,
        consts: Vec<Konst>,
        protos: Vec<Vec<u8>>,
    }

    fn lua_str(out: &mut Vec<u8>, s: &str) {
        out.extend(((s.len() + 1) as u64).to_le_bytes());
        out.extend(s.as_bytes());
        out.push(0);
    }

    /// One function: its code, constants and inner functions.
    fn function(params: u8, code: &[u32], consts: &[Konst], protos: &[Vec<u8>]) -> Vec<u8> {
        let mut o = vec![0u8; 8 + 8]; // no source name, no lines
        o.extend([0, params, 0, 20]);
        o.extend((code.len() as u32).to_le_bytes());
        code.iter().for_each(|c| o.extend(c.to_le_bytes()));
        o.extend((consts.len() as u32).to_le_bytes());
        for k in consts {
            match k {
                Konst::Nil => o.push(0),
                Konst::Bool(b) => o.extend([1, *b as u8]),
                Konst::Num(n) => {
                    o.push(3);
                    o.extend(n.to_le_bytes());
                }
                // paths are wide strings in NVIDIA's files
                Konst::Str(s) if s.contains('\\') => {
                    o.push(9);
                    o.extend((s.encode_utf16().count() as u64 + 1).to_le_bytes());
                    s.encode_utf16().chain([0]).for_each(|c| o.extend(c.to_le_bytes()));
                }
                Konst::Str(s) => {
                    o.push(4);
                    lua_str(&mut o, s);
                }
            }
        }
        o.extend((protos.len() as u32).to_le_bytes());
        protos.iter().for_each(|p| o.extend(p));
        o.extend([0u8; 12]); // no line info, locals, upvalue names
        o
    }

    impl Asm {
        fn k(&mut self, k: Konst) -> usize {
            self.consts.iter().position(|c| *c == k).unwrap_or_else(|| {
                self.consts.push(k);
                self.consts.len() - 1
            })
        }
        fn s(&mut self, s: &str) -> usize {
            self.k(Konst::Str(s.into()))
        }
        fn abc(&mut self, op: u32, a: usize, b: usize, c: usize) {
            self.code.push(op | (a as u32) << 6 | (c as u32) << 14 | (b as u32) << 23);
        }
        fn abx(&mut self, op: u32, a: usize, bx: usize) {
            self.code.push(op | (a as u32) << 6 | (bx as u32) << 14);
        }
        fn loadk(&mut self, a: usize, k: Konst) {
            let k = self.k(k);
            self.abx(1, a, k);
        }
        /// `R[a] = require(name)`
        fn require(&mut self, a: usize, name: &str) {
            let (g, n) = (self.s("require"), self.s(name));
            self.abx(5, a, g);
            self.abx(1, a + 1, n);
            self.abc(28, a, 2, 2);
        }
        /// `R[a] = {label, value}`
        fn pair(&mut self, a: usize, label: &str, value: Konst) {
            self.abc(10, a, 2, 0);
            self.loadk(a + 1, Konst::Str(label.into()));
            match value {
                Konst::Bool(b) => self.abc(2, a + 2, b as usize, 0),
                // NULL is a global in the wrappers
                Konst::Nil => {
                    let g = self.s("NULL");
                    self.abx(5, a + 2, g)
                }
                v => self.loadk(a + 2, v),
            }
            self.abc(34, a, 2, 1);
        }
        /// `R[ctx]:add_property{name=.., type=.., sources={R[parser]:ref(key) ...}, ...}`; `tail` fills the
        /// table in R[6] (free registers from 7).
        fn property(&mut self, ctx: usize, parser: usize, name: &str, ty: &str, keys: &[&str], tail: impl FnOnce(&mut Asm)) {
            let (add, n, t, src, rf) = (self.s("add_property"), self.s("name"), self.s("type"), self.s("sources"), self.s("ref"));
            let (name, ty) = (self.s(name), self.s(ty));
            self.abc(11, 4, ctx, 256 + add); // SELF: R4 = method, R5 = ctx
            self.abc(10, 6, 0, 4);
            self.abc(9, 6, 256 + n, 256 + name);
            self.abc(9, 6, 256 + t, 256 + ty);
            self.abc(10, 7, keys.len(), 0);
            for (i, key) in keys.iter().enumerate() {
                self.abc(11, 8 + i, parser, 256 + rf);
                self.loadk(10 + i, Konst::Str((*key).into()));
                // the last call returns "every result", as the compiler writes it
                self.abc(28, 8 + i, 3, if i + 1 == keys.len() { 0 } else { 2 });
            }
            self.abc(34, 7, 0, 1);
            self.abc(9, 6, 256 + src, 7);
            tail(self);
            self.abc(28, 4, 3, 1);
        }
        /// `values = {{label, value}, ...}` of the property being built.
        fn values(&mut self, rows: &[(&str, Konst)]) {
            self.abc(10, 7, rows.len(), 0);
            for (i, (label, value)) in rows.iter().enumerate() {
                self.pair(8 + i * 3, label, value.clone());
                self.abc(0, 20 + i, 8 + i * 3, 0); // MOVE the row next to the others
            }
            for i in 0..rows.len() {
                self.abc(0, 8 + i, 20 + i, 0);
            }
            self.abc(34, 7, rows.len(), 1);
            let v = self.s("values");
            self.abc(9, 6, 256 + v, 7);
        }
        /// `range = {min, max, precision, decode = <proto>, encode = <proto>}`
        fn range(&mut self, min: f64, max: f64, digits: f64, convert: &[u32]) {
            let p = self.protos.len();
            self.protos.push(function(1, convert, &[], &[]));
            self.abc(10, 7, 0, 5);
            for (k, v) in [("min", min), ("max", max), ("precision", digits)] {
                let (k, v) = (self.s(k), self.k(Konst::Num(v)));
                self.abc(9, 7, 256 + k, 256 + v);
            }
            for k in ["decode", "encode"] {
                let k = self.s(k);
                self.abx(36, 8, p);
                self.abc(9, 7, 256 + k, 8);
            }
            let r = self.s("range");
            self.abc(9, 6, 256 + r, 7);
        }
        pub(crate) fn chunk(mut self) -> Vec<u8> {
            self.abc(30, 0, 1, 0);
            let mut o = HEADER.to_vec();
            o.extend(function(0, &self.code, &self.consts, &self.protos));
            o
        }
    }

    const RETURN_ARG: [u32; 2] = [30 | 2 << 23, 30 | 1 << 23];
    pub(crate) const ROB_FILE: &str = "%profile%\\AppData\\LocalLow\\Kefir\\ROB RUN\\settings\\settings_steam_4262460_*.dat";

    /// A wrapper shaped like ROB RUN's: a JSON file found by pattern, plus the registry.
    pub(crate) fn rob_run() -> Vec<u8> {
        let mut a = Asm::default();
        a.require(0, "context_v1");
        a.require(1, "json_parser_v1");
        a.require(2, "glob_parser_v1");
        a.require(3, "reg_parser_v1");
        // Options = glob:new(json, glob.LAST_MODIFIED, path)
        let (new, last, options, regopts) = (a.s("new"), a.s("LAST_MODIFIED"), a.s("Options"), a.s("RegOpts"));
        a.abc(11, 4, 2, 256 + new);
        a.abc(0, 6, 1, 0);
        a.abc(6, 7, 2, 256 + last);
        a.loadk(8, Konst::Str(ROB_FILE.into()));
        a.abc(28, 4, 5, 2);
        a.abx(7, 4, options);
        a.abc(11, 4, 3, 256 + new);
        a.loadk(6, Konst::Str("HKCU\\Software\\Kefir\\ROB RUN".into()));
        a.abc(28, 4, 3, 2);
        a.abx(7, 4, regopts);
        // the parsers are globals from here on: R1 = Options, R3 = RegOpts
        a.abx(5, 1, options);
        a.abx(5, 3, regopts);
        let on_off = [("Disabled", Konst::Bool(false)), ("Disabled", Konst::Nil), ("Enabled", Konst::Bool(true))];
        a.property(0, 1, "Display Mode", "ENUM", &["fullscreen_setting", "fullscreen_setting"], |a| a.values(&[("Windowed", Konst::Bool(false))]));
        a.property(0, 1, "Render Resolution", "FLOAT", &["render_scale_setting"], |a| a.range(0.4, 2.0, 1.0, &RETURN_ARG));
        a.property(0, 1, "Anti-aliasing", "ENUM", &["anti_aliasing_mode_id_setting"], |a| a.values(&[("Disabled", Konst::Str("disabled".into())), ("SMAA", Konst::Str("smaa".into())), ("TAA", Konst::Str("taa".into()))]));
        a.property(0, 1, "HBAO", "ENUM", &["hbao_setting"], |a| a.values(&on_off));
        a.property(0, 1, "Shadows", "ENUM", &["cast_shadow_setting"], |a| a.values(&[("Disabled", Konst::Str("disabled".into())), ("Minimum Quality", Konst::Str("low".into()))]));
        a.property(0, 1, "Texture Quality", "ENUM", &["global_texture_mipmap_limit"], |a| a.values(&[("Low", Konst::Num(2.0)), ("Medium", Konst::Num(1.0)), ("High", Konst::Num(0.0)), ("High", Konst::Nil)]));
        a.property(0, 1, "Level of Detail", "FLOAT", &["lod_bias_setting"], |a| a.range(0.4, 2.0, 1.0, &RETURN_ARG));
        a.property(0, 1, "Grass", "ENUM", &["settings_grass"], |a| a.values(&[("Minimum Quality", Konst::Str("low".into())), ("High Quality", Konst::Str("high".into()))]));
        a.property(0, 3, "Registry Thing", "ENUM", &["Screenmanager Thing_h1"], |a| a.values(&[("Low", Konst::Num(0.0))]));
        // a number that a function converts (the function is more than `return v`)
        a.property(0, 1, "Sharpness", "FLOAT", &["sharpness_setting"], |a| a.range(0.0, 1.0, 1.0, &[12, 30 | 2 << 23]));
        a.chunk()
    }

    pub(crate) const ROB_POPS: &str = r#"{"settings":[{"Anti-aliasing":{"type":"ENUM"}},{"Display Mode":{"type":"ENUM"}},{"Grass":{"type":"ENUM"}},{"HBAO":{"type":"ENUM"}},{"Level of Detail":{"type":"FLOAT"}},{"Registry Thing":{"type":"ENUM"}},{"Render Resolution":{"type":"FLOAT"}},{"Shadows":{"type":"ENUM"}},{"Sharpness":{"type":"FLOAT"}},{"Texture Quality":{"type":"ENUM"}}],
        "pops":[{"values":{"1":"Disabled","2":"Windowed","3":"Minimum Quality","4":"Disabled","5":"0.4","6":"Low","7":"1.0","8":"Disabled","9":"0.5","10":"Low"}},{"values":{"1":"TAA","3":"High Quality","5":"2.0"}}]}"#;

    #[test]
    fn reads_constants_and_inner_functions_of_a_chunk() {
        let main = parse(&rob_run()).unwrap();
        let strings: Vec<&str> = main.consts.iter().filter_map(|k| if let Konst::Str(s) = k { Some(s.as_str()) } else { None }).collect();
        assert_eq!(strings[..6], ["require", "context_v1", "json_parser_v1", "glob_parser_v1", "reg_parser_v1", "new"]);
        assert!(strings.contains(&ROB_FILE), "wide strings are read");
        assert!(main.consts.contains(&Konst::Num(0.4)));
        assert_eq!((main.protos.len(), main.protos[0].params, main.protos[0].code.len()), (3, 1, 2));
    }

    #[test]
    fn fps_caps_and_binds_are_never_written() {
        for (name, key) in [("FPS limit", "x"), ("Frame Rate Cap", "x"), ("Key bindings", "x"), ("Mouse sensitivity", "x"), ("Limit", "FrameRateLimit"), ("Cap", "maxFps"), ("Jump", "key_key.jump")] {
            assert!(denied(name, key), "{name} / {key}");
        }
        assert!(!denied("Shadows", "shadow_quality") && !denied("Textures", "global_texture_mipmap_limit"));
    }

    #[test]
    fn lowest_keeps_only_plain_settings() {
        let (found, skipped) = lowest(&rob_run(), ROB_POPS.as_bytes());
        let got: Vec<(&str, &str, &str, bool)> = found.iter().map(|f| (f.label.as_str(), f.key.as_str(), f.best.as_str(), f.text)).collect();
        assert_eq!(
            got,
            [
                ("Anti-aliasing", "anti_aliasing_mode_id_setting", "disabled", true),
                ("HBAO", "hbao_setting", "false", false),
                ("Shadows", "cast_shadow_setting", "disabled", true),
                ("Texture Quality", "global_texture_mipmap_limit", "2", false),
                ("Level of Detail", "lod_bias_setting", "0.4", false),
                ("Grass", "settings_grass", "low", true),
            ]
        );
        assert!(found.iter().all(|f| f.file == ROB_FILE && !f.ini && f.section.is_empty()));
        assert_eq!(found[0].choices, [("Disabled".to_string(), "disabled".to_string()), ("SMAA".into(), "smaa".into()), ("TAA".into(), "taa".into())]);
        assert_eq!(found[1].choices, [("Disabled".to_string(), "false".to_string()), ("Enabled".into(), "true".into())], "the NULL row is how a missing key reads");
        assert_eq!(found[4].range, Some((0.4, 2.0, 1)));
        assert_eq!(
            skipped,
            ["Display Mode (several sources)", "Render Resolution (resolution, window or scaling)", "Registry Thing (not an ini or JSON file)", "Sharpness (number converted by a function)"]
        );
    }

    #[test]
    fn ini_sources_carry_their_section() {
        let mut a = Asm::default();
        a.require(0, "context_v1");
        a.require(2, "ini_parser_v1");
        let new = a.s("new");
        a.abc(11, 4, 2, 256 + new);
        a.loadk(6, Konst::Str("%localappdata%\\Game\\Saved\\GameUserSettings.ini".into()));
        a.abc(28, 4, 3, 2);
        a.abc(0, 1, 4, 0);
        a.property(0, 1, "VSync", "ENUM", &["/Script/G.Settings;bUseVSync"], |a| a.values(&[("Off", Konst::Str("False".into())), ("On", Konst::Str("True".into()))]));
        a.property(0, 1, "Shadows", "ENUM", &["ScalabilityGroups;sg.ShadowQuality"], |a| a.values(&[("Low", Konst::Num(0.0))]));
        a.property(0, 1, "Fog", "ENUM", &["NoSection"], |a| a.values(&[("Low", Konst::Str("0".into()))]));
        let pops = br#"{"settings":[{"Fog":{}},{"Shadows":{}},{"VSync":{}}],"pops":[{"values":{"1":"Low","2":"Low","3":"Off"}}]}"#;
        let (found, skipped) = lowest(&a.chunk(), pops);
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].ini, found[0].section.as_str(), found[0].key.as_str(), found[0].best.as_str()), (true, "/Script/G.Settings", "bUseVSync", "False"));
        assert_eq!(skipped, ["Shadows (mixed value types)", "Fog (no section)"]);
    }

    #[test]
    fn broken_input_gives_nothing_and_never_panics() {
        let good = rob_run();
        for cut in 0..good.len() {
            let _ = lowest(&good[..cut], ROB_POPS.as_bytes()); // every cut-off file
        }
        let mut bad = good.clone();
        for i in (12..bad.len()).step_by(7) {
            bad[i] ^= 0xA5; // and a damaged one
            let _ = lowest(&bad, ROB_POPS.as_bytes());
        }
        assert!(lowest(&good, b"{").0.is_empty());
        assert!(lowest(&good, br#"{"settings":[1],"pops":[]}"#).0.is_empty());
        assert!(lowest(b"\x1BLuaQ", ROB_POPS.as_bytes()).0.is_empty());
        // a jump in the main chunk: not straight-line code
        let mut a = Asm::default();
        a.abc(22, 0, 0, 0);
        assert_eq!(lowest(&a.chunk(), ROB_POPS.as_bytes()), (vec![], vec!["wrapper is not plain table code".to_string()]));
    }

    /// What this PC's NVIDIA App cache gives, game by game. Reads only.
    #[test]
    #[ignore]
    fn real_cache() {
        let root = PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap()).join("NVIDIA Corporation\\NVIDIA App\\NvBackend\\Recommendations");
        for d in fs::read_dir(&root).unwrap().flatten().filter(|d| d.path().is_dir()) {
            let slug = d.file_name().to_string_lossy().to_string();
            let (found, skipped) = for_game(&root, &crate::gamelib::title_key(&slug.replace('_', " ")));
            println!("== {slug}: {} settings; left out: {}", found.len(), skipped.join(", "));
            for f in found {
                println!("   {} -> [{}] {} = {}   ({})", f.label, f.section, f.key, f.best, f.file);
            }
        }
    }
}
