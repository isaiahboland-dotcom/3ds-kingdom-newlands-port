//! Loads what `tools/extract` writes (sprite pages, `sprites.json`,
//! `clips.json`) and draws `scene` through macroquad.

use std::path::Path;
use std::rc::Rc;

use ::scene::authored::{read_file, FILENAME};
use ::scene::cloud_shadow::{self, Geometry, MAP_H, MAP_W};
use ::scene::render::{sprite_corners, Color, Draw, Rect, Renderer};
use ::scene::sheet::Sheet;
use game::data::WorldData;
use macroquad::miniquad::{
    gl::{glDepthMask, GL_FALSE, GL_TRUE},
    BlendFactor, BlendState, BlendValue, Comparison, Equation, TextureAccess, TextureFormat,
    TextureParams, TextureSource,
};
use macroquad::prelude as mq;
use macroquad::prelude::{
    draw_circle, draw_circle_lines, draw_mesh, draw_rectangle, draw_rectangle_lines,
    draw_texture_ex, gl_use_default_material, gl_use_material, load_material, load_texture, vec2,
    vec3, DrawTextureParams, FilterMode, Mat4, Material, MaterialParams, Mesh, PipelineParams,
    ShaderSource, Texture2D, UniformDesc, UniformType, Vertex,
};

pub struct Assets {
    pub pages: Vec<Texture2D>,
    /// The same pages, linearly filtered (for `Draw::smooth`).
    smooth: Vec<Texture2D>,
    pub sheet: Rc<Sheet>,
    /// Recolors sprite pixels keyed by alpha. See `RECOLOR_FS`.
    recolor: DepthMaterials,
    /// Additive blending (glows, PostFX add color).
    additive: DepthMaterials,
    /// Water reflections (see `REFLECT_FS`).
    reflect: DepthMaterials,
    cloud: DepthMaterials,
    fog: Material,
    world_z: Option<f32>,
    depth_write_mask: bool,
    cloud_pass: cloud_shadow::Pass,
    cloud_depth: Option<CloudDepth>,
    shadow_camera: Option<cloud_shadow::Camera>,
    pub cloud_defs: Option<::scene::cloud_data::CloudsDef>,
    pub atmosphere: Option<::scene::atmosphere_data::AtmosphereDef>,
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

const DEPTH_WIDTH: u32 = MAP_W as u32;
const DEPTH_HEIGHT: u32 = MAP_H as u32;
const DEPTH_MAX: f32 = 16_777_215.0;

/// The receiver samples the shadow camera's encoded depth in its fragment
/// shader. Macroquad's wrapper for a raw texture does not own its allocation.
struct CloudDepth {
    texture: Texture2D,
    bytes: Vec<u8>,
}

impl CloudDepth {
    fn new() -> Self {
        let texture = unsafe { mq::get_internal_gl() }.quad_context.new_texture(
            TextureAccess::Static,
            TextureSource::Empty,
            TextureParams {
                format: TextureFormat::RGBA8,
                width: DEPTH_WIDTH,
                height: DEPTH_HEIGHT,
                min_filter: FilterMode::Nearest,
                mag_filter: FilterMode::Nearest,
                ..Default::default()
            },
        );
        Self {
            texture: Texture2D::from_miniquad_texture(texture),
            bytes: vec![0; (DEPTH_WIDTH * DEPTH_HEIGHT * 4) as usize],
        }
    }

    fn upload(&mut self, depth: &[f32]) {
        encode_depth(depth, &mut self.bytes);
        let id = self.texture.raw_miniquad_id();
        let mut gl = unsafe { mq::get_internal_gl() };
        // Finish any draws using the previous map before replacing it.
        gl.flush();
        gl.quad_context.texture_update_part(
            id,
            0,
            0,
            DEPTH_WIDTH as i32,
            DEPTH_HEIGHT as i32,
            &self.bytes,
        );
    }
}

impl Drop for CloudDepth {
    fn drop(&mut self) {
        let id = self.texture.raw_miniquad_id();
        unsafe { mq::get_internal_gl() }
            .quad_context
            .delete_texture(id);
    }
}

pub(crate) fn encode_depth(depth: &[f32], bytes: &mut [u8]) {
    assert_eq!(bytes.len(), depth.len() * 4);
    for (&value, out) in depth.iter().zip(bytes.as_chunks_mut::<4>().0) {
        let word = (value.clamp(0.0, 1.0) * DEPTH_MAX).round() as u32;
        let [_, r, g, b] = word.to_be_bytes();
        *out = [r, g, b, 255];
    }
}

const CLOUD_VS: &str = r#"#version 100
attribute vec3 position;
attribute vec2 texcoord;
attribute vec4 color0;
attribute vec4 normal;
varying highp vec2 uv;
varying highp vec4 shadow;
varying lowp vec4 color;
uniform mat4 Model;
uniform mat4 Projection;
void main() {
    gl_Position = Projection * Model * vec4(position, 1);
    color = color0 / 255.0;
    uv = texcoord;
    shadow = normal;
}
"#;

pub(crate) const CLOUD_FS: &str = r#"#version 100
precision lowp float;
varying highp vec2 uv;
varying highp vec4 shadow;
varying vec4 color;
uniform sampler2D Texture;
uniform highp sampler2D CloudDepth;
uniform vec4 CloudLight;
const highp vec2 depthSize = vec2(512.0, 256.0);
const highp vec3 depthWeights = vec3(65536.0, 256.0, 1.0);

highp float depthTexel(highp vec2 pixel) {
    highp vec2 coord = (pixel + 0.5) / depthSize;
    highp vec3 encoded = texture2D(CloudDepth, coord).rgb;
    return dot(encoded, depthWeights) / 65793.0;
}

highp float sampleDepth(highp vec2 pixel) {
    // Filter decoded RGB packing in the encoded camera-depth domain.
    highp vec2 corner = floor(pixel);
    highp vec2 t = pixel - corner;
    highp float a = mix(depthTexel(corner),
                        depthTexel(corner + vec2(1.0, 0.0)), t.x);
    highp float b = mix(depthTexel(corner + vec2(0.0, 1.0)),
                        depthTexel(corner + vec2(1.0, 1.0)), t.x);
    highp float depth = mix(a, b, t.y);
    return 1.0 / (depth * (1.0 / 20.0 - 1.0 / 0.3) + 1.0 / 0.3);
}

void main() {
    vec4 t = texture2D(Texture, uv);
    if (t.a == 0.0) {
        discard;
    }
    // The atlas stores (1 + source alpha) / 2 after the PC alpha cutoff.
    // Its complement is the unlit highlight's original alpha coefficient.
    float exposure = 1.0;
    if (shadow.w > 0.5 && t.a < 1.0) {
        highp vec2 pixel = shadow.xy;
        highp float a = sampleDepth(pixel);
        highp float b = sampleDepth(pixel + vec2(0.0, 2.0));
        exposure = (shadow.z - a < 0.05 ? 0.5 : 0.0)
                 + (shadow.z - b < 0.05 ? 0.5 : 0.0);
    }
    gl_FragColor = vec4(t.rgb * color.rgb + (1.0 - t.a) * CloudLight.rgb * exposure,
                        t.a * color.a);
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

fn world_clip_z(z: f32) -> f32 {
    // Keep subpixel depth offsets distinct in the native fog range (-5..-2)
    // on Macroquad's 16-bit attachment. Only fog writes in this range.
    ((z + 3.5) * 0.2).clamp(-0.95, 0.95)
}

struct DepthMaterials {
    painter: Material,
    world: Material,
}

impl DepthMaterials {
    fn load(
        vertex: &str,
        fragment: &str,
        uniforms: &[UniformDesc],
        textures: &[String],
        dst: BlendFactor,
    ) -> Result<Self, String> {
        Ok(Self {
            painter: material(vertex, fragment, uniforms, textures, dst, false)?,
            world: material(vertex, fragment, uniforms, textures, dst, true)?,
        })
    }

    fn get(&self, world_z: Option<f32>) -> &Material {
        if world_z.is_some() {
            &self.world
        } else {
            &self.painter
        }
    }
}

fn material(
    vertex: &str,
    fragment: &str,
    uniforms: &[UniformDesc],
    textures: &[String],
    dst: BlendFactor,
    depth_test: bool,
) -> Result<Material, String> {
    load_material(
        ShaderSource::Glsl { vertex, fragment },
        MaterialParams {
            uniforms: uniforms.to_vec(),
            textures: textures.to_vec(),
            pipeline_params: PipelineParams {
                color_blend: Some(BlendState::new(
                    Equation::Add,
                    BlendFactor::Value(BlendValue::SourceAlpha),
                    dst,
                )),
                // Miniquad's GL backend uses depth_write to enable the test.
                // The separate GL depth mask controls actual depth writes.
                depth_write: depth_test,
                depth_test: Comparison::LessOrEqual,
                ..Default::default()
            },
        },
    )
    .map_err(|e| e.to_string())
}

const FOG_FS: &str = r#"#version 100
precision highp float;
varying lowp vec2 uv;
varying lowp vec4 color;
uniform sampler2D Texture;
uniform float FogAlpha;
void main() {
    vec4 art = texture2D(Texture, uv);
    float coverage = art.a * FogAlpha;
    if (coverage < 0.01) {
        discard;
    }
    gl_FragColor = vec4(art.rgb * color.rgb, coverage);
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
    fn depth_writes(&mut self, write: bool) {
        if write != self.depth_write_mask {
            // Macroquad queues geometry, but the GL mask applies when it runs.
            // Submit the preceding draws before changing the mask.
            unsafe {
                mq::get_internal_gl().flush();
                glDepthMask(if write { GL_TRUE } else { GL_FALSE } as u8);
            }
            self.depth_write_mask = write;
        }
    }

    fn read_depth(&mut self) {
        if self.world_z.is_some() {
            self.depth_writes(false);
        }
    }

    fn indexed_sprite(
        &mut self,
        id: u32,
        mesh: Option<&::scene::cloud_mesh::CloudMesh>,
        at: (f32, f32),
        d: &Draw,
        tints: Option<&[Color]>,
    ) {
        let fog = d.depth_write && self.world_z.is_some();
        if fog {
            self.depth_writes(true);
        } else {
            self.read_depth();
        }
        let s = self.sheet.get(id);
        let geometry = Geometry::new(s, mesh.or_else(|| self.sheet.cloud_mesh(id)));
        let texture = &self.pages[usize::from(s.page)];
        let shadow = self.shadow_camera.zip(d.cloud_shadow);
        let vertices = geometry
            .vertices()
            .iter()
            .enumerate()
            .map(|(index, &local)| {
                let [x, y] = cloud_shadow::screen_point(local, at, d);
                let u = (f32::from(s.x) + s.px + local[0]) / texture.width();
                let v = (f32::from(s.y) + s.py + local[1]) / texture.height();
                let mut vertex = Vertex::new(
                    x,
                    y,
                    0.0,
                    u,
                    v,
                    mc(tints.map_or(d.tint, |colors| colors[index])),
                );
                if let Some((camera, receiver)) = shadow {
                    let affine = d.basis.unwrap_or([1.0, 0.0, 0.0, 1.0]);
                    let scale = [affine[0] * d.scale, affine[3] * d.scale];
                    let [x, y, z] =
                        cloud_shadow::world_point(local, receiver.position, scale, d.flip);
                    let [u, v, clip_z] = camera.project(x, y, z);
                    vertex.normal = mq::vec4(u, v, clip_z, 1.0);
                } else {
                    vertex.normal = mq::Vec4::ZERO;
                }
                vertex
            })
            .collect();
        if fog {
            gl_use_material(&self.fog);
            self.fog.set_uniform("FogAlpha", d.tint.a);
        } else if let Some(light) = d.cloud_light {
            let material = self.cloud.get(self.world_z);
            gl_use_material(material);
            material.set_uniform("CloudLight", mc(light).to_vec());
        }
        draw_mesh(&Mesh {
            vertices,
            indices: geometry.indices().to_vec(),
            texture: Some(texture.clone()),
        });
        gl_use_default_material();
    }

    pub async fn load(dir: &Path, world: &WorldData, world_id: u32) -> Result<Assets, String> {
        let read = |name: &str| {
            std::fs::read_to_string(dir.join(name))
                .map_err(|e| format!("{}: {e}", dir.join(name).display()))
        };
        let mut sheet = Sheet::from_json(
            &read("sprites.json")?,
            &read("clips.json")?,
            &read("font.json")?,
        )?;
        sheet.link(&world.sprites);
        let authored_path = dir.join(FILENAME);
        let authored_error = match read_file(&authored_path) {
            Ok(bytes) => sheet.install_authored(&bytes, world_id, world).err(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => Some(e.to_string()),
        };
        if let Some(e) = authored_error {
            eprintln!("ignoring {}: {e}", authored_path.display());
        }
        let cloud_path = dir.join(::scene::cloud_data::FILE_NAME);
        let cloud_defs = match ::scene::cloud_data::load(&cloud_path, world_id, &sheet) {
            Ok(def) => def,
            Err(error) => {
                eprintln!("ignoring {}: {error}", cloud_path.display());
                None
            }
        };
        if let Some(def) = &cloud_defs {
            let path = dir.join(::scene::cloud_mesh::FILE_NAME);
            match read_file(&path) {
                Ok(bytes) => {
                    if let Err(error) = sheet.link_cloud_meshes(&def.sprites, world_id, &bytes) {
                        eprintln!("ignoring {}: {error}", path.display());
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => eprintln!("ignoring {}: {error}", path.display()),
            }
        }
        let atmosphere = match ::scene::atmosphere_data::AtmosphereDef::load(
            &dir.join(::scene::atmosphere_data::FILE_NAME),
            world_id,
            &sheet,
        ) {
            Ok(def) => def,
            Err(e) => {
                eprintln!("ignoring fog and mist data ({e})");
                None
            }
        };
        let cloud_ids: Vec<_> = cloud_defs
            .iter()
            .flat_map(|def| &def.sprites)
            .filter_map(|name| sheet.id(name))
            .collect();
        let mut cloud_pass = cloud_shadow::Pass::default();
        let mut pages = Vec::new();
        let mut smooth = Vec::new();
        for (page, p) in sheet.pages.iter().enumerate() {
            let path = dir.join(&p.file);
            let tex = load_texture(&path.to_string_lossy())
                .await
                .map_err(|e| format!("{}: {e}", path.display()))?;
            tex.set_filter(FilterMode::Nearest);
            let image = tex.get_texture_data();
            for &id in &cloud_ids {
                let sprite = sheet.get(id);
                let end_x = usize::from(sprite.x) + usize::from(sprite.w);
                let end_y = usize::from(sprite.y) + usize::from(sprite.h);
                if usize::from(sprite.page) == page
                    && end_x <= usize::from(image.width)
                    && end_y <= usize::from(image.height)
                {
                    let alpha = (usize::from(sprite.y)..end_y).flat_map(|y| {
                        let start = (y * usize::from(image.width) + usize::from(sprite.x)) * 4;
                        let end = start + usize::from(sprite.w) * 4;
                        image.bytes[start..end]
                            .as_chunks::<4>()
                            .0
                            .iter()
                            .map(|pixel| pixel[3])
                    });
                    cloud_pass.insert(id, sprite, sheet.cloud_mesh(id), alpha);
                }
            }
            let soft = Texture2D::from_image(&image);
            soft.set_filter(FilterMode::Linear);
            smooth.push(soft);
            pages.push(tex);
        }
        let alpha_blend = BlendFactor::OneMinusValue(BlendValue::SourceAlpha);
        let recolor = DepthMaterials::load(
            RECOLOR_VS,
            RECOLOR_FS,
            &[
                UniformDesc::new("RecolorA", UniformType::Float4),
                UniformDesc::new("RecolorB", UniformType::Float4),
            ],
            &[],
            alpha_blend,
        )?;
        let cloud = DepthMaterials::load(
            CLOUD_VS,
            CLOUD_FS,
            &[UniformDesc::new("CloudLight", UniformType::Float4)],
            &["CloudDepth".into()],
            alpha_blend,
        )?;
        let additive = DepthMaterials::load(RECOLOR_VS, PLAIN_FS, &[], &[], BlendFactor::One)?;
        let reflect = DepthMaterials::load(RECOLOR_VS, REFLECT_FS, &[], &[], BlendFactor::One)?;
        let fog = material(
            RECOLOR_VS,
            FOG_FS,
            &[UniformDesc::new("FogAlpha", UniformType::Float1)],
            &[],
            alpha_blend,
            true,
        )?;
        let cloud_depth = cloud_defs.as_ref().map(|_| CloudDepth::new());
        let texture = cloud_depth
            .as_ref()
            .map_or_else(Texture2D::empty, |map| map.texture.clone());
        cloud.painter.set_texture("CloudDepth", texture.clone());
        cloud.world.set_texture("CloudDepth", texture);
        Ok(Assets {
            cloud,
            cloud_pass,
            cloud_depth,
            shadow_camera: None,
            fog,
            world_z: None,
            depth_write_mask: true,
            cloud_defs,
            atmosphere,
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
        self.world_depth(f32::NEG_INFINITY);
        mq::clear_background(mc(c));
    }

    fn world_depth(&mut self, z: f32) {
        let next = (z != f32::NEG_INFINITY).then(|| world_clip_z(z));
        if self.world_z == next {
            return;
        }
        if next.is_none() {
            // Restore the write mask before another target or a depth clear.
            self.depth_writes(true);
        }
        let gl = unsafe { mq::get_internal_gl() }.quad_gl;
        if self.world_z.is_some() {
            gl.pop_model_matrix();
        }
        if let Some(z) = next {
            gl.push_model_matrix(Mat4::from_translation(vec3(0.0, 0.0, z)));
        }
        gl.depth_test(next.is_some());
        self.world_z = next;
    }

    fn cloud_shadows(
        &mut self,
        camera: cloud_shadow::Camera,
        clouds: &[cloud_shadow::Sprite],
    ) -> bool {
        self.shadow_camera = None;
        let Some(texture) = &mut self.cloud_depth else {
            return false;
        };
        let Some(depth) = self.cloud_pass.prepare_depth(camera, clouds) else {
            return false;
        };
        texture.upload(depth);
        self.shadow_camera = Some(camera);
        true
    }

    fn sprite(&mut self, id: u32, at: (f32, f32), d: Draw) {
        if d.cloud_light.is_some() {
            self.indexed_sprite(id, None, at, &d, None);
            return;
        }
        self.read_depth();
        let s = *self.sheet.get(id);
        if let Some((a, b)) = d.recolor {
            let material = self.recolor.get(self.world_z);
            gl_use_material(material);
            material.set_uniform("RecolorA", mc(a).to_vec());
            material.set_uniform("RecolorB", mc(b).to_vec());
        } else if d.additive {
            gl_use_material(self.additive.get(self.world_z));
        }
        let pages = if d.smooth { &self.smooth } else { &self.pages };
        let texture = &pages[s.page as usize];
        if d.basis.is_some() || d.flip_y {
            let (pw, ph) = (texture.width(), texture.height());
            let (u0, u1) = (s.x as f32 / pw, (s.x + s.w) as f32 / pw);
            let (v0, v1) = (s.y as f32 / ph, (s.y + s.h) as f32 / ph);
            let (ul, ur) = if d.flip { (u1, u0) } else { (u0, u1) };
            let (vt, vb) = if d.flip_y { (v1, v0) } else { (v0, v1) };
            let vertices = sprite_corners(&s, at, &d)
                .into_iter()
                .zip([(ul, vt), (ur, vt), (ur, vb), (ul, vb)])
                .map(|((x, y), (u, v))| Vertex::new(x, y, 0.0, u, v, mc(d.tint)))
                .collect();
            draw_mesh(&Mesh {
                vertices,
                indices: vec![0, 1, 2, 0, 2, 3],
                texture: Some(texture.clone()),
            });
        } else {
            let px = if d.flip { s.w as f32 - s.px } else { s.px };
            let (mut x, mut y) = (at.0 - px * d.scale, at.1 - s.py * d.scale);
            if d.scale == 1.0 && d.angle == 0.0 {
                (x, y) = (x.round(), y.round());
            }
            draw_texture_ex(
                texture,
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
        }
        if d.recolor.is_some() || d.additive {
            gl_use_default_material();
        }
    }

    fn atmosphere_sprite(
        &mut self,
        id: u32,
        mesh: &::scene::cloud_mesh::CloudMesh,
        at: (f32, f32),
        draw: Draw,
        tints: Option<&[Color]>,
    ) {
        self.indexed_sprite(id, Some(mesh), at, &draw, tints);
    }

    /// orig: `Image.Type.Sliced`.
    fn sliced(&mut self, id: u32, dest: Rect, border: (f32, f32, f32, f32), tint: Color) {
        self.read_depth();
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
        self.read_depth();
        draw_rectangle(r.x, r.y, r.w, r.h, mc(c));
    }

    fn rect_lines(&mut self, r: Rect, thickness: f32, c: Color) {
        self.read_depth();
        draw_rectangle_lines(r.x, r.y, r.w, r.h, thickness, mc(c));
    }

    fn gradient(&mut self, r: Rect, top: Color, bottom: Color) {
        self.read_depth();
        gradient_mesh(r, top, bottom);
    }

    fn add(&mut self, r: Rect, c: Color) {
        self.read_depth();
        gl_use_material(self.additive.get(self.world_z));
        draw_rectangle(r.x, r.y, r.w, r.h, mc(c));
        gl_use_default_material();
    }

    fn add_gradient(&mut self, r: Rect, top: Color, bottom: Color) {
        self.read_depth();
        gl_use_material(self.additive.get(self.world_z));
        gradient_mesh(r, top, bottom);
        gl_use_default_material();
    }

    fn circle(&mut self, x: f32, y: f32, radius: f32, c: Color) {
        self.read_depth();
        draw_circle(x, y, radius, mc(c));
    }

    fn circle_lines(&mut self, x: f32, y: f32, radius: f32, thickness: f32, c: Color) {
        self.read_depth();
        draw_circle_lines(x, y, radius, thickness, mc(c));
    }

    fn reflect(&mut self, dest: Rect, src: Rect, tint: Color) {
        self.read_depth();
        let Some(t) = &self.previous else { return };
        gl_use_material(self.reflect.get(self.world_z));
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

#[cfg(test)]
mod depth_tests {
    use super::*;

    #[test]
    fn desktop_depth_keeps_near_far_and_coplanar_fog_distinct() {
        let depth16 = |z| ((world_clip_z(z) * 0.5 + 0.5) * 65535.0).round() as u16;
        for z in [-5.0, -3.5, -2.0] {
            assert!(depth16(z - 0.001) < depth16(z));
            assert!(depth16(z + 0.001) > depth16(z));
        }
        assert!(depth16(-100.0) < depth16(-5.0));
        assert!(depth16(100.0) > depth16(-2.0));
    }
}
