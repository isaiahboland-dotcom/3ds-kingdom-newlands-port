//! Shelf-packs cut sprites into pages of at most 1024×1024 (the 3DS texture
//! limit) and writes the pages plus `sprites.json`.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;

use crate::sprites::Sheet;

const PAGE: u32 = 1024;
/// Transparent gap between sprites so filtering never bleeds neighbours in.
const PAD: u32 = 1;

#[derive(Serialize)]
struct SpriteOut {
    page: usize,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    /// Pivot in pixels from the sprite's top-left.
    px: f32,
    py: f32,
    ppu: f32,
}

#[derive(Serialize)]
struct SheetOut {
    pages: Vec<String>,
    sprites: BTreeMap<String, SpriteOut>,
}

struct Page {
    w: u32,
    h: u32,
    px: Vec<u8>,
    shelf_y: u32,
    shelf_h: u32,
    cursor_x: u32,
}

impl Page {
    fn new(w: u32, h: u32) -> Self {
        Self {
            w,
            h,
            px: vec![0; (w * h * 4) as usize],
            shelf_y: 0,
            shelf_h: 0,
            cursor_x: 0,
        }
    }

    fn place(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        if self.cursor_x + w > self.w {
            self.shelf_y += self.shelf_h + PAD;
            self.shelf_h = 0;
            self.cursor_x = 0;
        }
        if self.shelf_y + h > self.h || w > self.w {
            return None;
        }
        let at = (self.cursor_x, self.shelf_y);
        self.cursor_x += w + PAD;
        self.shelf_h = self.shelf_h.max(h);
        Some(at)
    }
}

pub fn write_pages(sheet: &Sheet, dir: &Path, json_path: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;

    let mut order: Vec<usize> = (0..sheet.sprites.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse((sheet.sprites[i].img.h, sheet.sprites[i].img.w)));

    let mut pages: Vec<Page> = Vec::new();
    let mut out = SheetOut {
        pages: Vec::new(),
        sprites: BTreeMap::new(),
    };
    for i in order {
        let s = &sheet.sprites[i];
        let (w, h) = (s.img.w, s.img.h);
        if w > PAGE || h > PAGE {
            eprintln!(
                "warning: {} is {w}×{h}, larger than a {PAGE}² page; gets its own page",
                s.name
            );
        }
        let placed = pages
            .iter_mut()
            .enumerate()
            .find_map(|(n, p)| p.place(w, h).map(|xy| (n, xy)));
        let (n, (x, y)) = match placed {
            Some(v) => v,
            None => {
                let mut p = Page::new(PAGE.max(w), PAGE.max(h));
                let xy = p.place(w, h).expect("fresh page fits");
                pages.push(p);
                (pages.len() - 1, xy)
            }
        };
        let page = &mut pages[n];
        for row in 0..h {
            let src = (row * w * 4) as usize;
            let dst = (((y + row) * page.w + x) * 4) as usize;
            page.px[dst..dst + (w * 4) as usize]
                .copy_from_slice(&s.img.px[src..src + (w * 4) as usize]);
        }
        out.sprites.insert(
            s.name.clone(),
            SpriteOut {
                page: n,
                x,
                y,
                w,
                h,
                px: s.pivot.0,
                py: s.pivot.1,
                ppu: s.ppu,
            },
        );
    }

    for (n, p) in pages.iter().enumerate() {
        let name = format!("page_{n}.png");
        let path = dir.join(&name);
        let file = std::fs::File::create(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), p.w, p.h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().map_err(|e| e.to_string())?;
        w.write_image_data(&p.px).map_err(|e| e.to_string())?;
        out.pages.push(format!("sprites/{name}"));
    }
    // Remove atlas pages left by an earlier, larger conversion.
    for entry in std::fs::read_dir(dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if let Some(n) = name
            .strip_prefix("page_")
            .and_then(|s| s.strip_suffix(".png"))
            .and_then(|s| s.parse::<usize>().ok())
        {
            if n >= pages.len() {
                std::fs::remove_file(entry.path()).map_err(|e| e.to_string())?;
            }
        }
    }
    println!("packed into {} pages", pages.len());

    let json = serde_json::to_string(&out).map_err(|e| e.to_string())?;
    std::fs::write(json_path, json).map_err(|e| format!("{}: {e}", json_path.display()))
}
