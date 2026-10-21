//! Per-blob token cache under `.git/qgate/`, so unchanged files are never re-tokenized.

use std::collections::HashMap;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use anyhow::Result;

use super::tokens::Token;

/// Version 2 tokenizes generated Go files to nothing.
const MAGIC: &[u8] = b"QGDUP2\n";

/// Cache key: blob id plus how the file is tokenized.
pub type Key = String;

pub fn key(oid: &str, tag: &str) -> Key {
    format!("{oid}:{tag}")
}

#[derive(Default)]
pub struct TokenCache {
    path: PathBuf,
    entries: HashMap<Key, Vec<Token>>,
    dirty: bool,
}

impl TokenCache {
    /// Load the cache, starting empty if it is missing or corrupt.
    pub fn load(state_dir: &Path) -> TokenCache {
        let path = state_dir.join("dup-tokens.bin");
        let entries = std::fs::read(&path)
            .ok()
            .and_then(|b| decode(&b))
            .unwrap_or_default();
        TokenCache {
            path,
            entries,
            dirty: false,
        }
    }

    pub fn get(&self, key: &str) -> Option<&Vec<Token>> {
        self.entries.get(key)
    }

    pub fn insert(&mut self, key: Key, tokens: Vec<Token>) {
        self.entries.insert(key, tokens);
        self.dirty = true;
    }

    /// Write the cache if anything changed. When it has grown well past what
    /// the current tree uses, drop entries not in `live`.
    pub fn save(mut self, live: &std::collections::HashSet<Key>) -> Result<()> {
        if self.entries.len() > 2 * live.len() + 1000 {
            self.entries.retain(|k, _| live.contains(k));
            self.dirty = true;
        }
        if !self.dirty {
            return Ok(());
        }
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = self
            .path
            .with_extension(format!("tmp{}", std::process::id()));
        let mut w = BufWriter::new(std::fs::File::create(&tmp)?);
        w.write_all(MAGIC)?;
        w.write_all(&(self.entries.len() as u32).to_le_bytes())?;
        for (k, toks) in &self.entries {
            w.write_all(&(k.len() as u16).to_le_bytes())?;
            w.write_all(k.as_bytes())?;
            w.write_all(&(toks.len() as u32).to_le_bytes())?;
            for t in toks {
                w.write_all(&t.hash.to_le_bytes())?;
                w.write_all(&t.line.to_le_bytes())?;
            }
        }
        w.into_inner()?.sync_all()?;
        std::fs::rename(tmp, &self.path)?;
        Ok(())
    }
}

fn decode(b: &[u8]) -> Option<HashMap<Key, Vec<Token>>> {
    let mut r = Reader { b, pos: 0 };
    if r.take(MAGIC.len())? != MAGIC {
        return None;
    }
    let n = r.u32()?;
    let mut map = HashMap::with_capacity(n as usize);
    for _ in 0..n {
        let klen = u16::from_le_bytes(r.take(2)?.try_into().ok()?) as usize;
        let key = String::from_utf8(r.take(klen)?.to_vec()).ok()?;
        let count = r.u32()? as usize;
        if count > b.len() / 12 {
            return None;
        }
        let mut toks = Vec::with_capacity(count);
        for _ in 0..count {
            let hash = u64::from_le_bytes(r.take(8)?.try_into().ok()?);
            let line = r.u32()?;
            toks.push(Token { hash, line });
        }
        map.insert(key, toks);
    }
    (r.pos == b.len()).then_some(map)
}

struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.b.get(self.pos..self.pos.checked_add(n)?)?;
        self.pos += n;
        Some(s)
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
}
