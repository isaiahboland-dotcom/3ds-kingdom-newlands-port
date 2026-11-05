//! Loads what `tools/extract` writes (sprite pages, `sprites.json`,
//! `clips.json`) and draws `scene` through macroquad.

use std::path::Path;
use std::rc::Rc;

use ::scene::render::{Color, Draw, Rect, Renderer};
use ::scene::sheet::Sheet;
use macroquad::miniquad::{BlendFactor, BlendState, BlendValue, Equation};
use macroquad::prelude as mq;
use macroquad::prelude::{
    draw_circle, draw_circle_lines, draw_mesh, draw_rectangle, draw_rectangle_lines,
    draw_texture_ex, gl_use_default_material, gl_use_material, load_material, load_texture, vec2,
    DrawTextureParams, FilterMode, Material, MaterialParams, Mesh, PipelineParams, ShaderSource,
    Texture2D, UniformDesc, UniformType, Vertex,
};

pub struct Assets {
    pub pages: Vec<Texture2D>,
    /// The same pages, linearly filtered (for `Draw::smooth`).
    smooth: Vec<Texture2D>,
    pub sheet: Rc<Sheet>,
    /// Recolors sprite pixels keyed by alpha. See `RECOLOR_FS`.
    recolor: Material,
    /// Additive blending (glows, PostFX add color).
    additive: Material,
    /// Water reflections (see `REFLECT_FS`).
    reflect: Material,
    /// The previous frame's top screen, for `reflect`.
    pub previous: Option<Texture2D>,
}

fn mc(c: Color) -> mq::Color {
    mq::Color::new(c.r, c.g, c.b, c.a)
}

const RECOLOR_FS: &str = r#"#version 100
precision lowp float;
varying vec2 uv;
varying vec4 color;
uniform sampler2D Texture;
uniform vec4 RecolorA;
uniform vec4 RecolorB;
void main() {
    vec4 t = texture2D(Texture, uv);
    // The keyed pixels' red channel is their shade of the recolour.
    if (t.a > 0.66 && t.a < 0.72) {
        t = vec4(t.r * RecolorB.rgb, 1.0);
    } else if (t.a > 0.79 && t.a < 0.85) {
        t = vec4(t.r * RecolorA.rgb, 1.0);
    }
    gl_FragColor = t * color;
}
"#;

const PLAIN_FS: &str = r#"#version 100
precision lowp float;
varying vec2 uv;
varying vec4 color;
uniform sampler2D Texture;
void main() {
    gl_FragColor = texture2D(Texture, uv) * color;
}
"#;

/// The water's reflection: the texture's colour only (the previous frame's
/// alpha is whatever its blending left), added over the water.
const REFLECT_FS: &str = r#"#version 100
precision lowp float;
varying vec2 uv;
varying vec4 color;
uniform sampler2D Texture;
void main() {
    gl_FragColor = vec4(texture2D(Texture, uv).rgb * color.rgb, color.a);
}
"#;

const RECOLOR_VS: &str = r#"#version 100
attribute vec3 position;
attribute vec2 texcoord;
attribute vec4 color0;
varying lowp vec2 uv;
varying lowp vec4 color;
uniform mat4 Model;
uniform mat4 Projection;
void main() {
    gl_Position = Projection * Model * vec4(position, 1);
    color = color0 / 255.0;
    uv = texcoord;
}
"#;

impl Assets {
    /// `world_sprites`: `WorldData::sprites`, so the sheet can map its ids.
    pub async fn load(dir: &Path, world_sprites: &[String]) -> Result<Assets, String> {
        let read = |name: &str| {
            std::fs::read_to_string(dir.join(name))
                .map_err(|e| format!("{}: {e}", dir.join(name).display()))
        };
        let mut sheet = Sheet::from_json(
            &read("sprites.json")?,
            &read("clips.json")?,
            &read("font.json")?,
        )?;
        sheet.link(world_sprites);
        let mut pages = Vec::new();
        let mut smooth = Vec::new();
        for p in &sheet.pages {
            let path = dir.join(&p.file);
            let tex = load_texture(&path.to_string_lossy())
                .await
                .map_err(|e| format!("{}: {e}", path.display()))?;
            tex.set_filter(FilterMode::Nearest);
            let soft = Texture2D::from_image(&tex.get_texture_data());
            soft.set_filter(FilterMode::Linear);
            smooth.push(soft);
            pages.push(tex);
        }
        let blend = |dst: BlendFactor| PipelineParams {
            color_blend: Some(BlendState::new(
                Equation::Add,
                BlendFactor::Value(BlendValue::SourceAlpha),
                dst,
            )),
            ..Default::default()
        };
        let recolor = load_material(
            ShaderSource::Glsl {
                vertex: RECOLOR_VS,
                fragment: RECOLOR_FS,
            },
            MaterialParams {
                uniforms: vec![
                    UniformDesc::new("RecolorA", UniformType::Float4),
                    UniformDesc::new("RecolorB", UniformType::Float4),
                ],
                pipeline_params: blend(BlendFactor::OneMinusValue(BlendValue::SourceAlpha)),
                ..Default::default()
            },
        )
        .map_err(|e| e.to_string())?;
        let additive = load_material(
            ShaderSource::Glsl {
                vertex: RECOLOR_VS,
                fragment: PLAIN_FS,
            },
            MaterialParams {
                pipeline_params: blend(BlendFactor::One),
                ..Default::default()
            },
        )
        .map_err(|e| e.to_string())?;
        let reflect = load_material(
            ShaderSource::Glsl {
                vertex: RECOLOR_VS,
                fragment: REFLECT_FS,
            },
            MaterialParams {
                pipeline_params: blend(BlendFactor::One),
                ..Default::default()
            },
        )
        .map_err(|e| e.to_string())?;
        Ok(Assets {
            pages,
            reflect,
            smooth,
            sheet: Rc::new(sheet),
            recolor,
            additive,
            previous: None,
        })
    }
}

/// A rectangle shaded from `top` to `bottom`.
fn gradient_mesh(r: Rect, top: Color, bottom: Color) {
    let v = |x: f32, y: f32, c: Color| Vertex::new(x, y, 0.0, 0.0, 0.0, mc(c));
    draw_mesh(&Mesh {
        vertices: vec![
            v(r.x, r.y, top),
            v(r.x + r.w, r.y, top),
            v(r.x + r.w, r.y + r.h, bottom),
            v(r.x, r.y + r.h, bottom),
        ],
        indices: vec![0, 1, 2, 0, 2, 3],
        texture: None,
    });
}

impl Renderer for Assets {
    fn clear(&mut self, c: Color) {
        mq::clear_background(mc(c));
    }

    fn sprite(&mut self, id: u32, at: (f32, f32), d: Draw) {
        let s = *self.sheet.get(id);
        if let Some((a, b)) = d.recolor {
            gl_use_material(&self.recolor);
            self.recolor.set_uniform("RecolorA", mc(a).to_vec());
            self.recolor.set_uniform("RecolorB", mc(b).to_vec());
        } else if d.additive {
            gl_use_material(&self.additive);
        }
        let px = if d.flip { s.w as f32 - s.px } else { s.px };
        let (mut x, mut y) = (at.0 - px * d.scale, at.1 - s.py * d.scale);
        if d.scale == 1.0 && d.angle == 0.0 {
            (x, y) = (x.round(), y.round());
        }
        let page = if d.smooth { &self.smooth } else { &self.pages };
        draw_texture_ex(
            &page[s.page as usize],
            x,
            y,
            mc(d.tint),
            DrawTextureParams {
                source: Some(mq::Rect::new(
                    s.x as f32, s.y as f32, s.w as f32, s.h as f32,
                )),
                dest_size: Some(vec2(s.w as f32 * d.scale, s.h as f32 * d.scale)),
                rotation: d.angle,
                pivot: (d.angle != 0.0).then_some(vec2(at.0, at.1)),
                flip_x: d.flip,
                ..Default::default()
            },
        );
        if d.recolor.is_some() || d.additive {
            gl_use_default_material();
        }
    }

    /// orig: `Image.Type.Sliced`.
    fn sliced(&mut self, id: u32, dest: Rect, border: (f32, f32, f32, f32), tint: Color) {
        let s = *self.sheet.get(id);
        let (l, b, r, t) = border;
        let (sx, sy, sw, sh) = (s.x as f32, s.y as f32, s.w as f32, s.h as f32);
        let cols = [
            (0.0, l, 0.0, l),
            (l, sw - l - r, l, dest.w - l - r),
            (sw - r, r, dest.w - r, r),
        ];
        let rows = [
            (0.0, t, 0.0, t),
            (t, sh - t - b, t, dest.h - t - b),
            (sh - b, b, dest.h - b, b),
        ];
        for &(cx, cw, dx, dw) in &cols {
            for &(ry, rh, dy, dh) in &rows {
                if cw <= 0.0 || rh <= 0.0 || dw <= 0.0 || dh <= 0.0 {
                    continue;
                }
                draw_texture_ex(
                    &self.pages[s.page as usize],
                    dest.x + dx,
                    dest.y + dy,
                    mc(tint),
                    DrawTextureParams {
                        source: Some(mq::Rect::new(sx + cx, sy + ry, cw, rh)),
                        dest_size: Some(vec2(dw, dh)),
                        ..Default::default()
                    },
                );
            }
        }
    }

    fn rect(&mut self, r: Rect, c: Color) {
        draw_rectangle(r.x, r.y, r.w, r.h, mc(c));
    }

    fn rect_lines(&mut self, r: Rect, thickness: f32, c: Color) {
        draw_rectangle_lines(r.x, r.y, r.w, r.h, thickness, mc(c));
    }

    fn gradient(&mut self, r: Rect, top: Color, bottom: Color) {
        gradient_mesh(r, top, bottom);
    }

    fn add(&mut self, r: Rect, c: Color) {
        gl_use_material(&self.additive);
        draw_rectangle(r.x, r.y, r.w, r.h, mc(c));
        gl_use_default_material();
    }

    fn add_gradient(&mut self, r: Rect, top: Color, bottom: Color) {
        gl_use_material(&self.additive);
        gradient_mesh(r, top, bottom);
        gl_use_default_material();
    }

    fn circle(&mut self, x: f32, y: f32, radius: f32, c: Color) {
        draw_circle(x, y, radius, mc(c));
    }

    fn circle_lines(&mut self, x: f32, y: f32, radius: f32, thickness: f32, c: Color) {
        draw_circle_lines(x, y, radius, thickness, mc(c));
    }

    fn reflect(&mut self, dest: Rect, src: Rect, tint: Color) {
        let Some(t) = &self.previous else { return };
        gl_use_material(&self.reflect);
        draw_texture_ex(
            t,
            dest.x,
            dest.y,
            mc(tint),
            DrawTextureParams {
                source: Some(mq::Rect::new(
                    src.x,
                    src.y.min(src.y + src.h),
                    src.w,
                    src.h.abs(),
                )),
                dest_size: Some(vec2(dest.w, dest.h)),
                flip_y: src.h < 0.0,
                ..Default::default()
            },
        );
        gl_use_default_material();
    }
}
