//! What a backend draws with. Coordinates are screen pixels, y down, from
//! the screen's top-left (400×240 top, 320×240 touch screen).

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const WHITE: Color = Color::new(1.0, 1.0, 1.0, 1.0);
    pub const BLACK: Color = Color::new(0.0, 0.0, 0.0, 1.0);

    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Color {
        Color { r, g, b, a }
    }

    pub fn rgb(c: [f32; 3]) -> Color {
        Color::new(c[0], c[1], c[2], 1.0)
    }

    pub fn rgba(c: [f32; 4]) -> Color {
        Color::new(c[0], c[1], c[2], c[3])
    }

    pub fn bytes(c: [u8; 4]) -> Color {
        let f = |v: u8| v as f32 / 255.0;
        Color::new(f(c[0]), f(c[1]), f(c[2]), f(c[3]))
    }

    pub fn times(self, o: Color) -> Color {
        Color::new(self.r * o.r, self.g * o.g, self.b * o.b, self.a * o.a)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    pub fn contains(&self, p: (f32, f32)) -> bool {
        p.0 >= self.x && p.0 < self.x + self.w && p.1 >= self.y && p.1 < self.y + self.h
    }
}

/// How to draw a sprite.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Draw {
    pub flip: bool,
    pub flip_y: bool,
    /// Optional row-major affine basis in screen coordinates, about the pivot.
    pub basis: Option<[f32; 4]>,
    /// Add the highlight weighted by the inverse cloud texture coverage.
    pub cloud_light: Option<Color>,
    pub tint: Color,
    pub scale: f32,
    /// Radians, clockwise on screen, around the pivot.
    pub angle: f32,
    /// orig: `SpriteFX` `recolorA` / `recolorB`.
    pub recolor: Option<(Color, Color)>,
    pub additive: bool,
    /// Filtered smoothly when scaled (glows); everything else stays pixel-sharp.
    pub smooth: bool,
}

impl Draw {
    pub fn tint(tint: Color) -> Draw {
        Draw {
            flip: false,
            flip_y: false,
            basis: None,
            cloud_light: None,
            tint,
            scale: 1.0,
            angle: 0.0,
            recolor: None,
            additive: false,
            smooth: false,
        }
    }

    pub fn flip(mut self, flip: bool) -> Draw {
        self.flip = flip;
        self
    }

    pub fn recolor(mut self, a: Color, b: Color) -> Draw {
        self.recolor = Some((a, b));
        self
    }

    pub fn angle(mut self, angle: f32) -> Draw {
        self.angle = angle;
        self
    }

    pub fn cloud_light(mut self, light: Color) -> Draw {
        self.cloud_light = Some(Color::new(light.r, light.g, light.b, 1.0));
        self
    }

    pub fn scale(mut self, scale: f32) -> Draw {
        self.scale = scale;
        self
    }

    pub fn additive(mut self) -> Draw {
        self.additive = true;
        self
    }

    pub fn smooth(mut self) -> Draw {
        self.smooth = true;
        self
    }
}

/// Shared affine geometry, ordered top-left, top-right, bottom-right,
/// bottom-left. Renderer flips change the pivot and UVs together.
pub fn sprite_corners(s: &crate::sheet::Sprite, at: (f32, f32), d: &Draw) -> [(f32, f32); 4] {
    let px = if d.flip { s.w as f32 - s.px } else { s.px };
    let py = if d.flip_y { s.h as f32 - s.py } else { s.py };
    let rel = [
        (-px, -py),
        (s.w as f32 - px, -py),
        (s.w as f32 - px, s.h as f32 - py),
        (-px, s.h as f32 - py),
    ];
    let [a, b, c, e] = d.basis.unwrap_or([1.0, 0.0, 0.0, 1.0]);
    let (sin, cos) = d.angle.sin_cos();
    let mut corners = rel.map(|(x, y)| {
        let (x, y) = ((a * x + b * y) * d.scale, (c * x + e * y) * d.scale);
        (at.0 + x * cos - y * sin, at.1 + x * sin + y * cos)
    });
    if d.scale == 1.0 && d.angle == 0.0 && [a, b, c, e] == [1.0, 0.0, 0.0, 1.0] {
        for p in &mut corners {
            *p = (p.0.round(), p.1.round());
        }
    }
    corners
}

/// Implemented by each backend (macroquad on desktop, citro3d on 3DS).
/// Calls arrive in painter's order.
pub trait Renderer {
    fn clear(&mut self, c: Color);
    /// Sprite `id` (in the backend's `Sheet`) with its pivot at `at`.
    fn sprite(&mut self, id: u32, at: (f32, f32), d: Draw);
    /// 9-sliced into `dest`; `border` is (left, bottom, right, top) pixels.
    fn sliced(&mut self, id: u32, dest: Rect, border: (f32, f32, f32, f32), tint: Color);
    fn rect(&mut self, r: Rect, c: Color);
    fn rect_lines(&mut self, r: Rect, thickness: f32, c: Color);
    /// A vertical gradient.
    fn gradient(&mut self, r: Rect, top: Color, bottom: Color);
    /// Adds `c` over `r`. orig: `PostFX` additive colour.
    fn add(&mut self, r: Rect, c: Color);
    /// `add` with a vertical gradient from `top` to `bottom`. Backends
    /// without one add the average colour.
    fn add_gradient(&mut self, r: Rect, top: Color, bottom: Color) {
        let mid = |a: f32, b: f32| (a + b) / 2.0;
        self.add(
            r,
            Color::new(
                mid(top.r, bottom.r),
                mid(top.g, bottom.g),
                mid(top.b, bottom.b),
                mid(top.a, bottom.a),
            ),
        );
    }
    fn circle(&mut self, x: f32, y: f32, radius: f32, c: Color);
    fn circle_lines(&mut self, x: f32, y: f32, radius: f32, thickness: f32, c: Color);
    /// Pixels `src` of the previous frame's top screen (a negative `h` flips
    /// them) stretched into `dest`, multiplied by `tint` and *added* over
    /// what's there (the source's own alpha is ignored). orig: the
    /// `MainCamera` reflection buffer the `Water` samples. Backends without
    /// one draw nothing.
    fn reflect(&mut self, _dest: Rect, _src: Rect, _tint: Color) {}
    /// Stereo depth of what's drawn next: 0 at the screen, 1 farthest
    /// behind it, negative in front. Backends without 3D ignore it.
    fn depth(&mut self, _d: f32) {}
}
