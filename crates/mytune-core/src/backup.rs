//! Backup records, byte-compatible with the C# `RegBackup` / `TweakBackup` JSON in state.json.
//!
//! Encoding (same as C#): Kind = enum name; DWord/QWord = decimal string; Binary = base64;
//! MultiString = `Multi` array with `Value` null; String/ExpandString = raw text (not expanded).
//! A value that did not exist is `Existed:false` with Kind/Value/Multi null.

use crate::error::{Error, Result};
use crate::reg::{self, Hive, RegValue};
use crate::util::{b64_decode, b64_encode};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;

/// A `null` reads as empty: one such field must not make the whole state.json unreadable.
pub(crate) fn null_default<'de, D: Deserializer<'de>, T: Default + Deserialize<'de>>(d: D) -> std::result::Result<T, D::Error> {
    Ok(Option::deserialize(d)?.unwrap_or_default())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct RegBackup {
    pub hive: String,
    pub path: String,
    pub name: String,
    pub existed: bool,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub multi: Option<Vec<String>>,
    /// Keys that did not exist before the write, shallowest first.
    #[serde(default, deserialize_with = "null_default")]
    pub created_keys: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct TweakBackup {
    #[serde(default, deserialize_with = "null_default")]
    pub title: String,
    #[serde(default, deserialize_with = "null_default")]
    pub applied_at: String,
    #[serde(default, deserialize_with = "null_default")]
    pub registry: Vec<RegBackup>,
    #[serde(default, deserialize_with = "null_default")]
    pub data: BTreeMap<String, String>,
    /// An apply failed and undoing it failed too: part of the change is still on the PC.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub partly_applied: bool,
}

pub fn to_parts(v: &RegValue) -> (&'static str, Option<String>, Option<Vec<String>>) {
    match v {
        RegValue::Dword(d) => ("DWord", Some(d.to_string()), None),
        // C# reads QWord with Convert.ToInt64, so it must be the signed decimal (u64::MAX -> "-1").
        RegValue::Qword(q) => ("QWord", Some((*q as i64).to_string()), None),
        RegValue::String(s) => ("String", Some(s.clone()), None),
        RegValue::ExpandString(s) => ("ExpandString", Some(s.clone()), None),
        RegValue::MultiString(m) => ("MultiString", None, Some(m.clone())),
        RegValue::Binary(b) => ("Binary", Some(b64_encode(b)), None),
    }
}

pub fn from_parts(kind: &str, value: Option<&str>, multi: Option<&[String]>) -> Result<RegValue> {
    let bad = |m: &str| Error::BadBackup(format!("{kind}: {m}"));
    let text = || value.ok_or_else(|| bad("Value is null"));
    Ok(match kind {
        "DWord" => {
            let t = text()?;
            // C# stores the unsigned value, but accept a signed one too (-1 => 0xFFFFFFFF).
            RegValue::Dword(t.parse::<u32>().or_else(|_| t.parse::<i32>().map(|i| i as u32)).map_err(|_| bad("bad number"))?)
        }
        "QWord" => {
            let t = text()?;
            RegValue::Qword(t.parse::<i64>().map(|i| i as u64).or_else(|_| t.parse::<u64>()).map_err(|_| bad("bad number"))?)
        }
        "String" => RegValue::String(text()?.to_string()),
        "ExpandString" => RegValue::ExpandString(text()?.to_string()),
        "MultiString" => RegValue::MultiString(multi.ok_or_else(|| bad("Multi is null"))?.to_vec()),
        "Binary" => RegValue::Binary(b64_decode(text()?).ok_or_else(|| bad("bad base64"))?),
        _ => return Err(bad("unknown kind")),
    })
}

impl RegBackup {
    fn hive(&self) -> Result<Hive> {
        Hive::parse(&self.hive).ok_or_else(|| Error::BadBackup(format!("unknown hive {}", self.hive)))
    }

    fn same_target(&self, hive: &str, path: &str, name: &str) -> bool {
        self.hive.eq_ignore_ascii_case(hive) && self.path.eq_ignore_ascii_case(path) && self.name.eq_ignore_ascii_case(name)
    }

    /// Puts the original back: rewrites the value, or deletes it (and keys we created, deepest first,
    /// only if empty) when it did not exist.
    pub fn restore(&self) -> Result<()> {
        let hive = self.hive()?;
        if self.existed {
            let kind = self.kind.as_deref().ok_or_else(|| Error::BadBackup("Existed but Kind is null".into()))?;
            let v = from_parts(kind, self.value.as_deref(), self.multi.as_deref())?;
            reg::set(hive, &self.path, &self.name, &v)
        } else {
            reg::delete_value(hive, &self.path, &self.name)?;
            for key in self.created_keys.iter().rev() {
                reg::delete_key_if_empty(hive, key)?;
            }
            Ok(())
        }
    }
}

impl TweakBackup {
    pub fn new(title: &str) -> TweakBackup {
        TweakBackup { title: title.to_string(), applied_at: crate::util::iso_now(), ..Default::default() }
    }

    /// Records the current state of one value. Call BEFORE writing it.
    /// Only the first capture of a value counts, so re-applying never overwrites the true original.
    pub fn capture(&mut self, hive: Hive, path: &str, name: &str) -> Result<()> {
        if self.registry.iter().any(|b| b.same_target(hive.name(), path, name)) {
            return Ok(());
        }
        let entry = match reg::get(hive, path, name)? {
            Some(v) => {
                let (kind, value, multi) = to_parts(&v);
                RegBackup {
                    hive: hive.name().into(),
                    path: path.into(),
                    name: name.into(),
                    existed: true,
                    kind: Some(kind.into()),
                    value,
                    multi,
                    created_keys: vec![],
                }
            }
            None => RegBackup {
                hive: hive.name().into(),
                path: path.into(),
                name: name.into(),
                existed: false,
                kind: None,
                value: None,
                multi: None,
                created_keys: reg::missing_keys(hive, path)?,
            },
        };
        self.registry.push(entry);
        Ok(())
    }

    /// Restores every captured value, newest first. A failure does not stop the others;
    /// afterwards all failures are returned together and the caller must keep this backup.
    pub fn restore_registry(&self) -> Result<()> {
        let mut failures = Vec::new();
        for b in self.registry.iter().rev() {
            if let Err(e) = b.restore() {
                let msg = format!("{}\\{}\\{}: {e}", b.hive, b.path, b.name);
                crate::log::error(&format!("restore failed {msg}"), None);
                failures.push(msg);
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(Error::Revert(failures))
        }
    }

    pub fn is_empty(&self) -> bool {
        self.registry.is_empty() && self.data.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parts_roundtrip() {
        let vals = [
            RegValue::Dword(4294967295),
            RegValue::Qword(u64::MAX),
            RegValue::String("a\\b".into()),
            RegValue::ExpandString("%x%".into()),
            RegValue::MultiString(vec!["a".into(), "b".into()]),
            RegValue::Binary(vec![1, 2, 3, 4]),
        ];
        for v in vals {
            let (k, val, multi) = to_parts(&v);
            assert_eq!(from_parts(k, val.as_deref(), multi.as_deref()).unwrap(), v);
        }
        assert_eq!(from_parts("DWord", Some("-1"), None).unwrap(), RegValue::Dword(u32::MAX));
        assert_eq!(to_parts(&RegValue::Qword(u64::MAX)).1.as_deref(), Some("-1"));
        assert_eq!(from_parts("QWord", Some("-1"), None).unwrap(), RegValue::Qword(u64::MAX));
        assert_eq!(from_parts("QWord", Some("18446744073709551615"), None).unwrap(), RegValue::Qword(u64::MAX));
        assert!(from_parts("DWord", None, None).is_err());
        assert!(from_parts("Nope", Some("1"), None).is_err());
    }

    /// The real entry from the C# state.json (adv.timer).
    #[test]
    fn reads_csharp_json() {
        let j = r#"{
          "Title": "0.5 ms timer resolution while gaming",
          "AppliedAt": "2026-11-09T23:28:34.9960836+01:00",
          "Registry": [{"Hive":"HKLM","Path":"SYSTEM\\CurrentControlSet\\Control\\Session Manager\\kernel",
            "Name":"GlobalTimerResolutionRequests","Existed":true,"Kind":"DWord","Value":"1","Multi":null,"CreatedKeys":[]}],
          "Data": {"enabled":"1"}
        }"#;
        let b: TweakBackup = serde_json::from_str(j).unwrap();
        assert_eq!(b.applied_at, "2026-11-09T23:28:34.9960836+01:00");
        assert_eq!(b.registry[0].kind.as_deref(), Some("DWord"));
        assert_eq!(b.data["enabled"], "1");
        let again = serde_json::to_value(&b).unwrap();
        assert_eq!(again["Registry"][0]["Multi"], serde_json::Value::Null);
        assert_eq!(again["Registry"][0]["CreatedKeys"], serde_json::json!([]));
    }
}
