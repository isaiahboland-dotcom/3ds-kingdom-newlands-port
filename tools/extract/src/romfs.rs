//! Builds a RomFS image (the "level 3" layout, without the IVFC wrapper) from
//! a folder: what devkitPro's `3dsxtool --romfs` appends to a `.3dsx`.
//!
//! Layout: a 0x28-byte header, then the directory hash table, directory
//! table, file hash table, file table, and the file data (each file aligned
//! to 4 bytes). Entries are named in UTF-16 and found through a hash of
//! their parent and name, chained per bucket.

use std::path::Path;

const NONE: u32 = 0xFFFF_FFFF;

struct Dir {
    parent: usize,
    name: String,
    sibling: u32,
    child_dir: u32,
    child_file: u32,
    next_hash: u32,
    off: u32,
}

struct File {
    parent: usize,
    name: String,
    sibling: u32,
    data_off: u64,
    data: Vec<u8>,
    next_hash: u32,
    off: u32,
}

/// Orders one folder's entries (names). Sorted by default.
pub type Order = fn(&mut [String]);

fn sorted(names: &mut [String]) {
    names.sort();
}

/// The image for everything under `root`, with entries in name order.
pub fn build(root: &Path) -> Result<Vec<u8>, String> {
    build_ordered(root, sorted)
}

/// Like [`build`], with the folder's entries ordered by `order`.
pub fn build_ordered(root: &Path, order: Order) -> Result<Vec<u8>, String> {
    let mut dirs = vec![Dir {
        parent: 0,
        name: String::new(),
        sibling: NONE,
        child_dir: NONE,
        child_file: NONE,
        next_hash: NONE,
        off: 0,
    }];
    let mut files = Vec::new();
    walk(root, 0, order, &mut dirs, &mut files)?;

    // Offsets in the tables.
    let entry_len =
        |fixed: usize, name: &str| fixed + (name.encode_utf16().count() * 2).div_ceil(4) * 4;
    let mut off = 0;
    for d in &mut dirs {
        d.off = off;
        off += entry_len(24, &d.name) as u32;
    }
    let dir_table = off;
    off = 0;
    for f in &mut files {
        f.off = off;
        off += entry_len(32, &f.name) as u32;
    }
    let file_table = off;

    // Sibling and child links hold table offsets: patch the indexes written
    // while walking.
    let dir_off: Vec<u32> = dirs.iter().map(|d| d.off).collect();
    let file_off: Vec<u32> = files.iter().map(|f| f.off).collect();
    let fix = |v: u32, offs: &[u32]| if v == NONE { NONE } else { offs[v as usize] };
    for d in &mut dirs {
        d.sibling = fix(d.sibling, &dir_off);
        d.child_dir = fix(d.child_dir, &dir_off);
        d.child_file = fix(d.child_file, &file_off);
    }
    for f in &mut files {
        f.sibling = fix(f.sibling, &file_off);
    }

    // Hash tables.
    let dir_buckets = bucket_count(dirs.len());
    let file_buckets = bucket_count(files.len());
    let mut dir_hash = vec![NONE; dir_buckets];
    let mut file_hash = vec![NONE; file_buckets];
    for d in &mut dirs {
        let parent = if d.off == 0 { 0 } else { dir_off[d.parent] };
        let b = (hash(parent, &d.name) as usize) % dir_buckets;
        d.next_hash = dir_hash[b];
        dir_hash[b] = d.off;
    }
    for f in &mut files {
        let b = (hash(dir_off[f.parent], &f.name) as usize) % file_buckets;
        f.next_hash = file_hash[b];
        file_hash[b] = f.off;
    }

    // File data.
    let mut data_len = 0u64;
    for f in &mut files {
        f.data_off = data_len;
        data_len += (f.data.len() as u64).div_ceil(4) * 4;
    }

    let dir_hash_off = 0x28u32;
    let dir_table_off = dir_hash_off + dir_buckets as u32 * 4;
    let file_hash_off = dir_table_off + dir_table;
    let file_table_off = file_hash_off + file_buckets as u32 * 4;
    let data_off = file_table_off + file_table;

    let mut out = Vec::new();
    let mut put = |v: u32| out.extend_from_slice(&v.to_le_bytes());
    for v in [
        0x28,
        dir_hash_off,
        dir_buckets as u32 * 4,
        dir_table_off,
        dir_table,
        file_hash_off,
        file_buckets as u32 * 4,
        file_table_off,
        file_table,
        data_off,
    ] {
        put(v);
    }
    for h in &dir_hash {
        out.extend_from_slice(&h.to_le_bytes());
    }
    for d in &dirs {
        let parent = if d.off == 0 { 0 } else { dir_off[d.parent] };
        for v in [parent, d.sibling, d.child_dir, d.child_file, d.next_hash] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        name(&mut out, &d.name);
    }
    for h in &file_hash {
        out.extend_from_slice(&h.to_le_bytes());
    }
    for f in &files {
        out.extend_from_slice(&dir_off[f.parent].to_le_bytes());
        out.extend_from_slice(&f.sibling.to_le_bytes());
        out.extend_from_slice(&f.data_off.to_le_bytes());
        out.extend_from_slice(&(f.data.len() as u64).to_le_bytes());
        out.extend_from_slice(&f.next_hash.to_le_bytes());
        name(&mut out, &f.name);
    }
    for f in &files {
        out.extend_from_slice(&f.data);
        while !out.len().is_multiple_of(4) {
            out.push(0);
        }
    }
    Ok(out)
}

fn name(out: &mut Vec<u8>, name: &str) {
    let units: Vec<u16> = name.encode_utf16().collect();
    out.extend_from_slice(&(units.len() as u32 * 2).to_le_bytes());
    for u in &units {
        out.extend_from_slice(&u.to_le_bytes());
    }
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
}

fn walk(
    path: &Path,
    dir: usize,
    order: Order,
    dirs: &mut Vec<Dir>,
    files: &mut Vec<File>,
) -> Result<(), String> {
    let mut names = Vec::new();
    for e in std::fs::read_dir(path).map_err(|e| format!("{}: {e}", path.display()))? {
        let e = e.map_err(|e| e.to_string())?;
        names.push(
            e.file_name()
                .into_string()
                .map_err(|n| format!("{n:?}: not UTF-8"))?,
        );
    }
    order(&mut names);
    for n in names {
        let p = path.join(&n);
        if p.is_dir() {
            let i = dirs.len();
            dirs.push(Dir {
                parent: dir,
                name: n,
                sibling: dirs[dir].child_dir,
                child_dir: NONE,
                child_file: NONE,
                next_hash: NONE,
                off: 1, // not the root
            });
            dirs[dir].child_dir = i as u32;
            walk(&p, i, order, dirs, files)?;
        } else {
            let data = std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?;
            files.push(File {
                parent: dir,
                name: n,
                sibling: dirs[dir].child_file,
                data_off: 0,
                data,
                next_hash: NONE,
                off: 0,
            });
            dirs[dir].child_file = (files.len() - 1) as u32;
        }
    }
    Ok(())
}

fn hash(parent: u32, name: &str) -> u32 {
    let mut h = parent ^ 123_456_789;
    for u in name.encode_utf16() {
        h = h.rotate_right(5) ^ u as u32;
    }
    h
}

fn bucket_count(n: usize) -> usize {
    if n < 3 {
        3
    } else if n < 19 {
        n | 1
    } else {
        let mut n = n;
        while [2, 3, 5, 7, 11, 13, 17]
            .iter()
            .any(|p| n.is_multiple_of(*p))
        {
            n += 1;
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Looks `path` up the way the console does: through the hash tables.
    fn find(img: &[u8], path: &str) -> Option<Vec<u8>> {
        let u32_at = |o: usize| u32::from_le_bytes(img[o..o + 4].try_into().unwrap());
        let u64_at = |o: usize| u64::from_le_bytes(img[o..o + 8].try_into().unwrap());
        let h = |i: usize| u32_at(i * 4) as usize;
        let (dh, dhs, dt, fh, fhs, fo, fd) = (h(1), h(2), h(3), h(5), h(6), h(7), h(9));
        let mut parent = 0u32;
        let parts: Vec<&str> = path.split('/').collect();
        for (i, part) in parts.iter().enumerate() {
            let last = i + 1 == parts.len();
            let (table, buckets, tab_off, fixed) = if last {
                (fh, fhs / 4, fo, 32)
            } else {
                (dh, dhs / 4, dt, 24)
            };
            let mut cur = u32_at(table + (hash(parent, part) as usize % buckets) * 4);
            loop {
                if cur == NONE {
                    return None;
                }
                let e = tab_off + cur as usize;
                let (par, next_at, nlen_at) = if last {
                    (u32_at(e), e + 24, e + 28)
                } else {
                    (u32_at(e), e + 16, e + 20)
                };
                let nlen = u32_at(nlen_at) as usize;
                let units: Vec<u16> = img[e + fixed..e + fixed + nlen]
                    .chunks(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .collect();
                if par == parent && String::from_utf16(&units).unwrap() == *part {
                    if last {
                        let off = u64_at(e + 8) as usize;
                        let len = u64_at(e + 16) as usize;
                        return Some(img[fd + off..fd + off + len].to_vec());
                    }
                    parent = cur;
                    break;
                }
                cur = u32_at(next_at);
            }
        }
        None
    }

    #[test]
    fn files_are_found_through_the_hashes() {
        let dir = std::env::temp_dir().join(format!("romfs-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("gfx/deep")).unwrap();
        let mut expect = Vec::new();
        for i in 0..40 {
            let p = format!("gfx/page_{i}.bin");
            std::fs::write(dir.join(&p), vec![i as u8; i * 7 + 1]).unwrap();
            expect.push((p, vec![i as u8; i * 7 + 1]));
        }
        std::fs::write(dir.join("gfx/deep/x.bin"), b"deep").unwrap();
        expect.push(("gfx/deep/x.bin".into(), b"deep".to_vec()));
        std::fs::write(dir.join("world.bin"), b"world!").unwrap();
        expect.push(("world.bin".into(), b"world!".to_vec()));
        let img = build(&dir).unwrap();
        for (p, data) in &expect {
            assert_eq!(find(&img, p).as_deref(), Some(&data[..]), "{p}");
        }
        assert_eq!(find(&img, "gfx/nope.bin"), None);
        std::fs::remove_dir_all(&dir).ok();
    }
}
