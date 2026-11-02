//! Top-screen world rendering: sky, celestial bodies, parallax layers,
//! lighting, everyone on the island, and the text overlays.

use game::build::{scaffold_filled, under_construction};
use game::coins::Motion;
use game::data::{sample_keys, FireDef, Layer, Role};
use game::greed::{TrollState, TROLL_Z};
use game::lighting::{celestial_position, color, Light};
use game::monarch::Action;
use game::units::{Anim, ShieldStatus};

use crate::heraldry;
use crate::render::{Color, Draw, Rect, Renderer};
use crate::session::Session;
use crate::sheet::{Clip, Sheet};

pub const W: f32 = 400.0;
pub const H: f32 = 240.0;
/// orig: `ScreenManager.PixelsPerUnit`.
pub const PPU: f32 = 32.0;

pub use game::build::WALK_Y;
/// orig: Main Camera y. The view is centered here.
pub const CAMERA_Y: f32 = 1.875;
/// Depth of the monarch and coins in the game layer. orig: Player prefab z;
/// `Droppable.Drop` puts coins 0.01 in front of the dropper.
const MONARCH_Z: f32 = 1.0;
const COIN_Z: f32 = 0.99;

/// Gradient quads of the main scene: (z, bottom y, top y), in world units.
/// orig: Level/Heaven/Sky, Level/Fill, Level/HazeFar, Level/HazeNear.
const SKY: (f32, f32, f32) = (64.0, 0.0, 6.0);
const FILL: (f32, f32, f32) = (12.0, 0.0, 6.0);
const HAZE_FAR: (f32, f32, f32) = (4.0, 0.0, 3.5);
const HAZE_NEAR: (f32, f32, f32) = (2.1, 0.0, 3.5);

const WHITE: Color = Color::WHITE;
pub(crate) const SKIN: Color = Color::new(0.775, 0.611, 0.581, 1.0);
/// orig: Ghost prefab `ghostColor`.
const GHOST_COLOR: Color = Color::new(0.819, 0.904, 0.798, 1.0);

/// Crown position when a clip doesn't key it (the same for king and queen).
/// orig: Horse Regular prefab, `King/Crown` transform.
pub(crate) const CROWN_DEFAULT_POS: (f32, f32) = (0.0625, 1.28125);

/// What's being drawn with: the session, the sprites and the backend.
struct Ctx<'a, R: Renderer> {
    s: &'a Session,
    sheet: &'a Sheet,
    r: &'a mut R,
}

impl<R: Renderer> Ctx<'_, R> {
    fn named(&mut self, name: &str, at: (f32, f32), d: Draw) {
        if let Some(id) = self.sheet.id(name) {
            self.r.sprite(id, at, d);
        }
    }

    fn world(&mut self, id: u16, at: (f32, f32), d: Draw) {
        if let Some(i) = self.sheet.world(id) {
            self.r.sprite(i, at, d);
        }
    }

    fn clip(&self, name: &str) -> Option<&Clip> {
        self.sheet.clip(name)
    }

    /// Root sprite of clip `name` at `t`.
    fn frame(&self, name: &str, t: f32) -> Option<u32> {
        self.clip(name).and_then(|c| c.sprite("", t))
    }

    fn light(&self) -> &Light {
        &self.s.game.blender.state
    }

    fn lit(&self, tint: Color) -> Color {
        lit(tint, self.light())
    }
}

/// Clip for each action. orig: `player_king` animator states via the steed's
/// override controller (`player_horse`, `player_bear`, ...).
pub(crate) fn clip_name(prefix: &str, a: Action) -> String {
    let state = match a {
        Action::Stand => "idle",
        Action::Walk => "walk",
        Action::Run => "gallop",
        Action::Eat => "eat",
        Action::Rear => "rearing",
    };
    format!("{prefix}_{state}")
}

/// orig: `Steed.Awake`: recolorB = coat, recolorA = mane (the coat of
/// arms' secondary colour for the warhorse while ridden).
pub(crate) fn steed_colors(s: &Session, steed: u16, ridden: bool) -> (Color, Color) {
    let d = &s.world.steeds[steed as usize];
    let mane = if ridden && d.recolor_to_coat {
        Color::rgb(s.game.coat.secondary)
    } else {
        Color::rgb(d.mane)
    };
    (mane, Color::rgb(d.coat))
}

/// Multiplies a sprite tint by the ambient light. orig: lit `Pow-*` materials.
/// Night is lifted a little (`NIGHT_LIFT` of the way to full light), since
/// handheld screens show the original's darkest nights as near black.
fn lit(tint: Color, light: &Light) -> Color {
    let a = light.colors[color::AMBIENT].map(|v| v + (1.0 - v) * NIGHT_LIFT);
    Color::new(tint.r * a[0], tint.g * a[1], tint.b * a[2], tint.a)
}

const NIGHT_LIFT: f32 = 0.07;

/// Screen pixel of a world point in a layer with parallax `ratio`.
pub fn layer_to_screen(s: &Session, x: f32, y: f32, ratio: f32) -> (f32, f32) {
    (
        ((x - ratio * s.camera_x) * PPU + W / 2.0).round(),
        (H / 2.0 - (y - CAMERA_Y) * PPU).round(),
    )
}

/// Screen pixel of a simulation position (ground-relative y, game layer).
pub fn world_to_screen(s: &Session, x: f32, y: f32) -> (f32, f32) {
    layer_to_screen(s, x, y + WALK_Y, 1.0)
}

/// Screen row of a world height.
pub fn screen_y(y: f32) -> f32 {
    (H / 2.0 - (y - CAMERA_Y) * PPU).round()
}

fn offset(p: (f32, f32), dx: f32, dy: f32) -> (f32, f32) {
    (p.0 + (dx * PPU).round(), p.1 - (dy * PPU).round())
}

/// Stereo depth of something `z` units behind the game layer: 0 at the
/// screen, toward 1 far behind it, negative in front (README Phase 5).
fn stereo_depth(z: f32) -> f32 {
    if !z.is_finite() {
        return 0.0;
    }
    (z / (z.abs() + 10.0)).clamp(-0.6, 1.0)
}

/// Full-width vertical gradient between world heights. orig: `HorizontalGradient`.
fn gradient(r: &mut impl Renderer, bottom_y: f32, top_y: f32, bottom: Color, top: Color) {
    let (yb, yt) = (screen_y(bottom_y), screen_y(top_y));
    r.gradient(Rect::new(0.0, yt, W, yb - yt), top, bottom);
}

pub enum DrawItem {
    Part(usize, usize),
    /// Part `p` of scenery object `o` (`Level::scenery`).
    Scenery(usize, usize),
    /// Scaffold module `m` of object `o`, shown while it's under construction.
    Scaffold(usize, usize),
    /// Empty price slots over the payable the monarch is at.
    PriceSlots,
    Unit(usize),
    Tool(usize),
    Troll(usize),
    Arrow(usize),
    Crown,
    Coin(usize),
    Monarch,
    Fill,
    HazeFar,
    HazeNear,
    Water,
    Celestial(usize),
    Glow(usize),
    /// A grass patch.
    Grass(usize),
    FreeSteed(usize),
    Hermit(usize),
    Bolt(usize),
    Merchant(usize),
    Mule(usize),
    Chest(usize),
    Banker,
    Boulder(usize),
    Dog,
    Animal(usize),
    /// A boat part lying at the wharf (or flying into place).
    LoosePart(usize),
    /// Someone standing on the boat's deck.
    Deckhand(usize),
    /// Fire `f` of object `o` (of the scenery when `scenery`).
    Fire { o: usize, f: usize, scenery: bool },
    /// The monarch's torch.
    Torch,
}

fn layer_z(world: &game::data::WorldData) -> f32 {
    world.layer(Layer::Game).z
}

/// Painter's order: Unity sorts by sorting layer, then order, then distance.
pub fn draw_list(s: &Session) -> Vec<((i16, i16, f32), DrawItem)> {
    let world = &s.world;
    let g = &s.game;
    let level = &g.level;
    let mut items = Vec::new();
    let view = W / PPU / 2.0 + 12.0;
    let cam = s.camera_x;
    for (oi, o) in level.objects.iter().enumerate() {
        let layer = world.layer(o.layer);
        if (o.x - layer.ratio * cam).abs() > view {
            continue;
        }
        let proto = &world.protos[o.proto as usize];
        // These are drawn from their clips instead.
        if proto.steed.is_some()
            || proto.hermit.is_some()
            || proto.merchant.is_some()
            || proto.chest.is_some()
        {
            continue;
        }
        // orig: WorkableBuilding.Start: under construction, the building is
        // hidden and its Scaffolding shown in its place.
        let building = under_construction(world, o);
        if building {
            if let Some(b) = &proto.building {
                for (mi, m) in b.scaffold.iter().enumerate() {
                    let z = layer.z + o.z + m.z;
                    items.push(((0, 0, -z), DrawItem::Scaffold(oi, mi)));
                }
            }
        }
        if !building {
            push_fires(&mut items, s, o, oi, false);
        }
        for (pi, part) in proto.parts.iter().enumerate() {
            // Under construction only the Scaffolding subtree shows, once built only the rest.
            if !o.parts[pi].visible || part.scaffold != building {
                continue;
            }
            let z = layer.z + o.z + part.z;
            items.push((
                (part.sort_layer, part.sort_order, -z),
                DrawItem::Part(oi, pi),
            ));
        }
    }
    for (oi, o) in level.scenery.iter().enumerate() {
        let layer = world.layer(o.layer);
        if (o.x - layer.ratio * cam).abs() > view {
            continue;
        }
        push_fires(&mut items, s, o, oi, true);
        for (pi, part) in world.protos[o.proto as usize].parts.iter().enumerate() {
            if !o.parts[pi].visible || part.scaffold {
                continue;
            }
            let mut z = layer.z + o.z + part.z;
            // The ghost waits where the forest grows, so it's drawn just
            // behind the monarch rather than lost behind a trunk.
            if g.tutorial.ghost.as_ref().is_some_and(|gh| gh.object == o.id) {
                z = MONARCH_Z - 0.02;
            }
            items.push((
                (part.sort_layer, part.sort_order, -z),
                DrawItem::Scenery(oi, pi),
            ));
        }
    }
    if let Some(gr) = &world.grass {
        let z = world.layer(Layer::Game).z + gr.z;
        for (i, p) in g.meadows.patches.iter().enumerate() {
            if p.alive && (p.x - cam).abs() < view {
                items.push(((0, 0, -z), DrawItem::Grass(i)));
            }
        }
    }
    let near = |id: u32| {
        level
            .index_of(id)
            .map(|j| &level.objects[j])
            .filter(|o| (o.x - cam).abs() < view)
    };
    for (i, f) in g.mounts.free.iter().enumerate() {
        if let Some(o) = near(f.object) {
            items.push(((0, 0, -(layer_z(world) + o.z)), DrawItem::FreeSteed(i)));
        }
    }
    for (i, h) in g.hermits.list.iter().enumerate() {
        if let Some(o) = near(h.object) {
            // Riding ones sit just behind the monarch.
            let z = if h.state == game::hermits::HermitState::Riding {
                MONARCH_Z + 0.001
            } else {
                layer_z(world) + o.z
            };
            items.push(((0, 0, -z), DrawItem::Hermit(i)));
        }
    }
    for (i, m) in g.merchants.list.iter().enumerate() {
        if let Some(o) = near(m.object) {
            let z = layer_z(world) + o.z;
            items.push(((0, 0, -z), DrawItem::Merchant(i)));
            items.push(((0, 0, -(z + 0.01)), DrawItem::Mule(i)));
        }
    }
    for (i, c) in g.treasure.chests.iter().enumerate() {
        if let Some(o) = near(c.object) {
            items.push(((0, 0, -(layer_z(world) + o.z)), DrawItem::Chest(i)));
        }
    }
    if let Some(b) = &g.treasure.banker {
        if !b.hidden() && (b.x - cam).abs() < view {
            items.push(((0, 0, -0.76), DrawItem::Banker));
        }
    }
    if let Some(d) = &g.dogs.dog {
        if (d.x - cam).abs() < view {
            items.push(((0, 0, -0.95), DrawItem::Dog));
        }
    }
    for i in 0..g.siege.boulders.len() {
        items.push(((0, 0, -0.7), DrawItem::Boulder(i)));
    }
    for i in 0..g.greed.bolts.len() {
        items.push(((0, 0, -0.7), DrawItem::Bolt(i)));
    }
    for (i, a) in g.wildlife.animals.iter().enumerate() {
        if (a.x - cam).abs() < view {
            items.push(((0, 0, -a.z), DrawItem::Animal(i)));
        }
    }
    for (i, c) in g.coins.list.iter().enumerate() {
        if (c.x - cam).abs() < view {
            items.push(((0, 0, -COIN_Z), DrawItem::Coin(i)));
        }
    }
    if let Some(w) = &g.harbor.wharf {
        for i in 0..w.stacked.len() + w.placing.len() {
            items.push(((0, 0, -(layer_z(world) - 0.004)), DrawItem::LoosePart(i)));
        }
    }
    if let Some(b) = &g.harbor.boat {
        if let Some(o) = level.index_of(b.object).map(|j| &level.objects[j]) {
            for (i, u) in b.deck.iter().enumerate() {
                let z = layer_z(world) + o.z - 0.002 - u.z * 0.001;
                items.push(((0, 0, -z), DrawItem::Deckhand(i)));
            }
        }
    }
    items.push(((0, 0, -MONARCH_Z), DrawItem::Monarch));
    if torch_lit(s) {
        items.push(((0, 0, -(MONARCH_Z - 0.01)), DrawItem::Torch));
    }
    for (i, u) in g.population.units.iter().enumerate() {
        if (u.x - cam).abs() < view {
            items.push(((0, 0, -u.z), DrawItem::Unit(i)));
        }
    }
    for (i, t) in g.greed.trolls.iter().enumerate() {
        if (t.x - cam).abs() < view {
            items.push(((0, 0, -TROLL_Z - i as f32 * 0.0001), DrawItem::Troll(i)));
        }
    }
    for i in 0..g.greed.arrows.len() {
        items.push(((0, 0, -0.7), DrawItem::Arrow(i)));
    }
    if g.greed.crown.is_some() {
        items.push(((0, 0, -0.9), DrawItem::Crown));
    }
    for (i, t) in g.population.tools.iter().enumerate() {
        if (t.x - cam).abs() < view {
            // orig: PayableShop.AddItem puts items just in front of the shop.
            items.push(((0, 0, -1.4), DrawItem::Tool(i)));
        }
    }
    // Coin indicators float in front of the scene.
    items.push(((0, 0, f32::INFINITY), DrawItem::PriceSlots));
    items.push(((0, 0, -FILL.0), DrawItem::Fill));
    items.push(((0, 0, -HAZE_FAR.0), DrawItem::HazeFar));
    items.push(((0, 0, -HAZE_NEAR.0), DrawItem::HazeNear));
    items.push(((0, 0, -world.water.z), DrawItem::Water));
    for (i, c) in world.celestials.iter().enumerate() {
        items.push(((0, 0, -c.z), DrawItem::Celestial(i)));
        for z in &c.glow_z {
            items.push(((0, 0, -z), DrawItem::Glow(i)));
        }
    }
    items.sort_by(|a, b| {
        a.0 .0
            .cmp(&b.0 .0)
            .then(a.0 .1.cmp(&b.0 .1))
            .then(a.0 .2.total_cmp(&b.0 .2))
    });
    items
}

/// Whether fire `f` burns now. orig: `Fire.Start` / `HandleOnDayEnd`:
/// night-only fires burn from dusk to dawn; ones that don't start
/// themselves (the statues') stay out.
fn burning(s: &Session, f: &FireDef) -> bool {
    f.auto_start && (!f.night_only || !s.game.days.is_daytime)
}

/// orig: `Player.Update`: the torch is lit when the ambient light is dim.
fn torch_lit(s: &Session) -> bool {
    s.world.steeds[s.game.steed as usize].torch.is_some()
        && s.game.blender.state.ambient_grayscale() < s.world.fire.torch_threshold
}

/// Draw items for an object's burning fires, above its own parts.
fn push_fires(items: &mut Vec<((i16, i16, f32), DrawItem)>, s: &Session, o: &game::level::Object, oi: usize, scenery: bool) {
    let proto = &s.world.protos[o.proto as usize];
    if proto.fires.is_empty() {
        return;
    }
    let layer = s.world.layer(o.layer);
    let top = proto.parts.iter().map(|p| p.sort_layer).max().unwrap_or(0);
    for (f, fire) in proto.fires.iter().enumerate() {
        if burning(s, fire) {
            let z = layer.z + o.z + fire.at.2;
            items.push(((top, i16::MAX, -z), DrawItem::Fire { o: oi, f, scenery }));
        }
    }
}

/// A fire: its flame, and its glow (a wide warm pool for the real lights:
/// the castle's campfire and the monarch's torch). orig: `Fire.SlowUpdate`
/// (intensity from strength, flickering by up to ±0.3 in the dark).
fn draw_fire<R: Renderer>(c: &mut Ctx<R>, at: (f32, f32), fire: &FireDef, seed: u32) {
    let s = c.s;
    let st = &s.world.fire;
    let size = (fire.size.0 * fire.size.0 + fire.size.1 * fire.size.1).sqrt();
    let strength = sample_keys(&st.strength, size);
    let dark = sample_keys(&st.ambient, s.game.blender.state.ambient_grayscale());
    // A smooth random flicker, stepping every 0.15 s like the original.
    let k = s.t / 0.15 + seed as f32 * 0.37;
    let noise = |i: f32| {
        let h = ((i as u32).wrapping_mul(0x9E37_79B9) ^ seed.wrapping_mul(0x85EB_CA6B)) >> 9;
        h as f32 / (1u32 << 23) as f32 * 2.0 - 1.0
    };
    let flicker = noise(k.floor()) + (noise(k.floor() + 1.0) - noise(k.floor())) * k.fract();
    let intensity = sample_keys(&st.intensity, strength) + flicker * 0.3 * dark;

    // The flame: the prefab nearest in size that has a sprite animation.
    let flame = st
        .flames
        .iter()
        .filter(|f| f.1.is_some())
        .min_by(|a, b| {
            let d = |f: &((f32, f32), Option<u16>)| (f.0 .0 - fire.size.0).powi(2) + (f.0 .1 - fire.size.1).powi(2);
            d(a).total_cmp(&d(b))
        });
    if let Some(name) = flame.and_then(|f| f.1).map(|id| s.world.sprites[id as usize].as_str()) {
        if let Some(sp) = flame_clip(c, name, s.t + seed as f32 * 0.13) {
            c.r.sprite(sp, at, Draw::tint(WHITE).additive());
        }
    }
    let Some(glow) = st.glow.and_then(|g| c.sheet.world(g)) else {
        return;
    };
    if fire.real_light {
        // orig: a point light of `rangeCurve` units; here a soft pool that
        // brightens what it covers, strongest in the dark.
        let range = sample_keys(&st.range, strength);
        let w = c.sheet.get(glow).w.max(1) as f32;
        let scale = range * 1.6 * PPU / w;
        let [r, g, b] = st.light_color;
        let a = (intensity / 4.0).clamp(0.0, 1.0) * 0.22 * dark;
        if a > 0.01 {
            c.r.sprite(glow, at, Draw::tint(Color::new(r * a, g * a, b * a, 1.0)).scale(scale).additive().smooth());
        }
    }
    // orig: without a real light, the glow sprite: alpha 0.4 × intensity,
    // scale 3 × intensity (on the child's own scale).
    let cur = intensity * 0.5 + 0.5;
    let [r, g, b, a] = st.glow_color;
    let alpha = (cur * 0.4 * a).clamp(0.0, 1.0);
    let tint = Color::new(r * alpha, g * alpha, b * alpha, 1.0);
    c.r.sprite(glow, at, Draw::tint(tint).scale(cur * 3.0 * st.glow_scale).additive().smooth());
}

/// The animation for flame sprite `name` (`fire_small_burn_4` →
/// `fire_small_burn`, `fire_mid_burn_7` → `fire_mid`).
fn flame_clip<R: Renderer>(c: &Ctx<R>, name: &str, t: f32) -> Option<u32> {
    let mut n = name.trim_end_matches(|ch: char| ch.is_ascii_digit()).trim_end_matches('_');
    loop {
        if let Some(sp) = c.frame(n, t) {
            return Some(sp);
        }
        n = &n[..n.rfind('_')?];
    }
}

fn draw_hermit<R: Renderer>(c: &mut Ctx<R>, i: usize) {
    use game::data::HermitKind;
    use game::hermits::HermitState;
    let s = c.s;
    let h = &s.game.hermits.list[i];
    let Some(o) = s
        .game
        .level
        .index_of(h.object)
        .map(|i| &s.game.level.objects[i])
    else {
        return;
    };
    let prefix = match h.kind {
        HermitKind::Baker => "hermit_baker",
        HermitKind::Ballista => "hermit_ballista",
        HermitKind::Knight => "hermit_knight",
    };
    let state = match h.state {
        HermitState::Riding => "ride",
        HermitState::Building { .. } => "build",
        _ if h.mover.vx.abs() > 0.05 => "walk",
        _ => "idle",
    };
    let Some(sprite) = c.frame(&format!("{prefix}_{state}"), h.anim_t) else {
        return;
    };
    // Riding: on the steed's Passenger spot.
    let (at, flip) = if h.state == HermitState::Riding {
        let m = &s.game.monarch;
        let d = &s.world.steeds[s.game.steed as usize];
        let f = m.facing as f32;
        (
            world_to_screen(s, m.x + d.passenger.0 * f, d.passenger.1),
            m.facing < 0,
        )
    } else {
        (world_to_screen(s, o.x, 0.0), o.flip)
    };
    let d = Draw::tint(c.lit(WHITE))
        .flip(flip)
        .recolor(Color::rgb(s.game.coat.primary), Color::rgb(h.skin));
    c.r.sprite(sprite, at, d);
}

/// The merchant (recolorA left at SpriteFX's default yellow, skin random)
/// or his mule (with or without its packs).
fn draw_merchant<R: Renderer>(c: &mut Ctx<R>, i: usize, mule: bool) {
    let s = c.s;
    let m = &s.game.merchants.list[i];
    let Some(o) = s
        .game
        .level
        .index_of(m.object)
        .map(|i| &s.game.level.objects[i])
    else {
        return;
    };
    let tint = c.lit(WHITE);
    if mule {
        let moving = m.mule.vx.abs() > 0.05;
        let clip = match (moving, m.mule_packed()) {
            (true, true) => "mule_walk",
            (true, false) => "mule_walk_no_pack",
            (false, true) => "mule_stand",
            (false, false) => "mule_stand_no_pack",
        };
        if let Some(sp) = c.frame(clip, s.t) {
            let at = world_to_screen(s, m.mule_x, 0.0);
            c.r.sprite(sp, at, Draw::tint(tint).flip(m.mule.facing < 0));
        }
        return;
    }
    let clip = if m.mover.vx.abs() > 0.05 {
        "merchant_walk"
    } else {
        "merchant_idle"
    };
    if let Some(sp) = c.frame(clip, m.anim_t) {
        let d = Draw::tint(tint)
            .flip(o.flip)
            .recolor(Color::new(1.0, 1.0, 0.0, 1.0), Color::rgb(m.skin));
        c.r.sprite(sp, world_to_screen(s, o.x, 0.0), d);
    }
}

fn draw_chest<R: Renderer>(c: &mut Ctx<R>, i: usize) {
    use game::treasure::ChestState;
    let s = c.s;
    let ch = &s.game.treasure.chests[i];
    let Some(o) = s
        .game
        .level
        .index_of(ch.object)
        .map(|i| &s.game.level.objects[i])
    else {
        return;
    };
    let clip = match ch.state {
        ChestState::Closed => "chest_close",
        ChestState::Opening { .. } | ChestState::Spilling { .. } => "chest_open",
        ChestState::Empty { .. } => "chest_empty",
    };
    if let Some(sp) = c.frame(clip, ch.anim_t) {
        let at = layer_to_screen(s, o.x, o.y, s.world.layer(o.layer).ratio);
        c.r.sprite(sp, at, Draw::tint(c.lit(WHITE)).flip(o.flip));
    }
}

/// orig: Banker SpriteFX: recolorA = coat of arms primary, recolorB = random skin.
fn draw_banker<R: Renderer>(c: &mut Ctx<R>) {
    let s = c.s;
    let Some(b) = &s.game.treasure.banker else {
        return;
    };
    let speed = b.mover.vx.abs();
    let clip = if speed > 0.9 {
        "banker_run"
    } else if speed > 0.05 {
        "banker_walk"
    } else {
        "banker_idle"
    };
    if let Some(sp) = c.frame(clip, b.anim_t) {
        let d = Draw::tint(c.lit(WHITE))
            .flip(b.mover.facing < 0)
            .recolor(Color::rgb(s.game.coat.primary), Color::rgb(b.skin));
        c.r.sprite(sp, world_to_screen(s, b.x, 0.0), d);
    }
}

fn draw_bolt<R: Renderer>(c: &mut Ctx<R>, i: usize) {
    let s = c.s;
    let b = &s.game.greed.bolts[i];
    let Some(sprite) = s.world.bolt_sprite else {
        return;
    };
    // orig: Bolt.LateUpdate points it along its velocity.
    let angle = (-b.vy).atan2(b.vx);
    let tint = c.lit(WHITE);
    c.world(
        sprite,
        world_to_screen(s, b.x, b.y),
        Draw::tint(tint).angle(angle),
    );
}

fn draw_free_steed<R: Renderer>(c: &mut Ctx<R>, i: usize) {
    use game::mounts::FreeState;
    let s = c.s;
    let f = &s.game.mounts.free[i];
    let Some(o) = s
        .game
        .level
        .index_of(f.object)
        .map(|i| &s.game.level.objects[i])
    else {
        return;
    };
    let d = &s.world.steeds[f.steed as usize];
    let action = match f.state {
        FreeState::Walking => Action::Walk,
        FreeState::Eating { .. } => Action::Eat,
        FreeState::Resting { .. } => Action::Stand,
    };
    let Some(sprite) = c.frame(&clip_name(&d.clips, action), f.anim_t) else {
        return;
    };
    let (mane, coat) = steed_colors(s, f.steed, false);
    let d = Draw::tint(c.lit(WHITE)).flip(o.flip).recolor(mane, coat);
    c.r.sprite(sprite, world_to_screen(s, o.x, 0.0), d);
}

fn draw_part<R: Renderer>(c: &mut Ctx<R>, o: &game::level::Object, pi: usize) {
    let s = c.s;
    let st = o.parts[pi];
    let Some(world_sprite) = st.sprite else {
        return;
    };
    let Some(mut sprite) = c.sheet.world(world_sprite) else {
        return;
    };
    let proto = &s.world.protos[o.proto as usize];
    // orig: PayableShield.status: the banner shows once a knight has the
    // shield, torn after the knight falls.
    let mut colors = (
        Color::rgb(s.game.coat.primary),
        Color::rgb(s.game.coat.secondary),
    );
    if let Some(def) = proto.shield_shop {
        if def.banner_part == Some(pi as u16) {
            let shop = s
                .game
                .population
                .shields
                .iter()
                .find(|sh| sh.object == o.id);
            let pick = match shop.map(|sh| sh.status) {
                Some(ShieldStatus::Taken) => def.banner,
                Some(ShieldStatus::Broken) => def.torn,
                _ => return,
            };
            if let Some(p) = pick.and_then(|p| c.sheet.world(p)) {
                sprite = p;
            }
            if let Some(sh) = shop {
                colors = (Color::rgb(sh.colors.0), Color::rgb(sh.colors.1));
            }
        }
    }
    let part = &proto.parts[pi];
    let ratio = s.world.layer(o.layer).ratio;
    let dir = if o.flip { -1.0 } else { 1.0 };
    let (mut px, mut py) = (part.x, part.y);
    // orig: Boat.sail animator: Closed until SailAway, then Open → Flapping;
    // the emblem rides on the cloth.
    if let Some(sail) = proto.boat.as_ref().and_then(|b| b.sail) {
        let sail_t = s
            .game
            .harbor
            .boat
            .as_ref()
            .filter(|b| b.object == o.id && o.mode == game::boat::mode::SAILING)
            .map(|b| b.sail_t);
        let clip = match sail_t {
            None => ("boat_sail_closed", 0.0),
            Some(t) if t < 0.875 => ("boat_sail_open", t),
            Some(t) => ("boat_sail_flapping", t - 0.875),
        };
        let clip_def = c.clip(clip.0);
        if pi == sail as usize {
            if let Some(id) = clip_def.and_then(|k| k.sprite("", clip.1)) {
                sprite = id;
            }
        } else if part.parent == Some(sail) {
            if sail_t.is_none() {
                return;
            }
            if let Some(k) = clip_def.filter(|k| k.has_offsets("Emblem")) {
                let (ex, ey) = k.offset("Emblem", clip.1);
                let sp = &proto.parts[sail as usize];
                (px, py) = (sp.x + ex, sp.y + ey);
            }
        }
    }
    let mut tint = Color::bytes(st.color.unwrap_or(part.color));
    // orig: Banner.AssignColorAndPattern(): the coat of arms' pattern on the
    // cloth, and its emblem in the emblem color.
    let name = &s.world.sprites[world_sprite as usize];
    let look = s.game.look;
    let mut cloth = heraldry::is_cloth(name);
    let emblem = if name.starts_with(heraldry::PATTERN) {
        if let Some(id) = heraldry::pattern(c.sheet, look) {
            sprite = id;
        }
        false
    } else if name.starts_with(heraldry::EMBLEM) {
        if let Some(id) = heraldry::emblem(c.sheet, look) {
            sprite = id;
        }
        true
    } else {
        name == heraldry::BORDER_ICON
    };
    if emblem {
        let [r, g, b] = s.game.coat.emblem;
        (tint.r, tint.g, tint.b) = (tint.r * r, tint.g * g, tint.b * b);
    }
    // orig: Ghost's animator (ghost_King / ghost_Queen) and SpriteFX fade;
    // it's the last monarch (`PreviousPlayerModel`, `PreviousPlayerColor`),
    // in their color with a pale `ghostColor` for skin.
    if let Some(gh) = s.game.tutorial.ghost.as_ref().filter(|gh| gh.object == o.id) {
        let clip = if look.queen { "ghost_Queen" } else { "ghost_King" };
        if let Some(id) = c.frame(clip, gh.anim_t) {
            sprite = id;
        }
        tint.a *= gh.alpha.clamp(0.0, 1.0);
        colors.1 = GHOST_COLOR;
        cloth = true;
    }
    let at = layer_to_screen(s, o.x + dir * (px + st.shift), o.y + py, ratio);
    if part.lit {
        tint = c.lit(tint);
    }
    let mut d = Draw::tint(tint).flip(o.flip ^ st.flip);
    if part.recolor || cloth {
        // orig: Banner: recolorA = primary, recolorB = secondary (a cloth's
        // SpriteFX is recoloured whatever its material).
        d = d.recolor(colors.0, colors.1);
    }
    c.r.sprite(sprite, at, d);
}

fn draw_scaffold<R: Renderer>(c: &mut Ctx<R>, oi: usize, mi: usize) {
    let s = c.s;
    let o = &s.game.level.objects[oi];
    let Some(b) = &s.world.protos[o.proto as usize].building else {
        return;
    };
    let m = &b.scaffold[mi];
    let filled = mi < scaffold_filled(&s.world, o);
    let Some(sprite) = (if filled { m.fill } else { m.sprite }) else {
        return;
    };
    let dir = if o.flip { -1.0 } else { 1.0 };
    let ratio = s.world.layer(o.layer).ratio;
    let at = layer_to_screen(s, o.x + dir * m.x, o.y + m.y, ratio);
    let tint = c.lit(WHITE);
    c.world(sprite, at, Draw::tint(tint).flip(o.flip));
}

/// orig: `Payable.Select()` shows one `coinIndicator` per coin of the price.
fn draw_price_slots<R: Renderer>(c: &mut Ctx<R>) {
    let g = &c.s.game;
    let Some(id) = g.pay.selected() else { return };
    let Some(p) = g.payables.iter().find(|p| p.id == id) else {
        return;
    };
    for i in 0..p.price {
        let (cx, cy) = p.coin_slot(i);
        let at = world_to_screen(c.s, cx, cy);
        c.named("coin_indicator", at, Draw::tint(WHITE));
    }
}

/// Clip name prefix per role (the animator controllers' clip names).
fn role_clip(role: Role) -> &'static str {
    match role {
        Role::Beggar => "beggar",
        Role::Peasant => "peasant",
        Role::Worker => "worker",
        Role::Archer => "archer",
        Role::Farmer => "farmer",
        Role::Knight => "knight",
    }
}

fn draw_unit<R: Renderer>(c: &mut Ctx<R>, i: usize) {
    let u = &c.s.game.population.units[i];
    draw_unit_at(c, u, u.x, u.y);
}

/// Someone on deck, carried along by the boat.
fn draw_deckhand<R: Renderer>(c: &mut Ctx<R>, i: usize) {
    let s = c.s;
    let Some(b) = &s.game.harbor.boat else {
        return;
    };
    let u = &b.deck[i];
    if let Some((x, y)) = s.game.harbor.deck_pos(&s.game.level, u.x) {
        draw_unit_at(c, u, x, y);
    }
}

/// A loose boat part: settled on the pile, or flying to its place on the hull.
/// orig: `Wharf.StackPart()` / `PlacePart()` (`DOMove` + `DORotate`, 0.5 s).
fn draw_loose_part<R: Renderer>(c: &mut Ctx<R>, i: usize) {
    let s = c.s;
    let g = &s.game;
    let Some(w) = &g.harbor.wharf else { return };
    let Some(o) = g.level.index_of(w.object).map(|j| &g.level.objects[j]) else {
        return;
    };
    let Some(def) = s.world.protos[o.proto as usize].wharf.as_ref() else {
        return;
    };
    let (p, t) = match w.stacked.get(i) {
        Some(p) => (*p, None),
        None => match w.placing.get(i - w.stacked.len()) {
            Some(&(p, t)) => (p, Some(t)),
            None => return,
        },
    };
    let Some(&(part, cx, cy)) = def.parts.get(p.index as usize) else {
        return;
    };
    let proto = &s.world.protos[o.proto as usize];
    let Some(sprite) = o.parts[part as usize].sprite else {
        return;
    };
    let f = if o.flip { -1.0 } else { 1.0 };
    // The part's centroid on the pile, and where it belongs on the hull.
    let pile = (p.x, p.y + WALK_Y);
    let home = (
        o.x + f * (proto.parts[part as usize].x + cx),
        o.y + proto.parts[part as usize].y + cy,
    );
    let (k, angle) = match t {
        Some(t) => {
            let k = (t / 0.5).min(1.0);
            (k, p.angle * (1.0 - k))
        }
        None => (0.0, p.angle),
    };
    let (x, y) = (
        pile.0 + (home.0 - pile.0) * k,
        pile.1 + (home.1 - pile.1) * k,
    );
    // Rotate around the centroid: offset the pivot by the rotated centroid.
    // The loose piece is never mirrored. orig: BoatPart.CopyProperties.
    let a = angle.to_radians();
    let (rx, ry) = (cx * a.cos() - cy * a.sin(), cx * a.sin() + cy * a.cos());
    let at = layer_to_screen(s, x - rx, y - ry, 1.0);
    let tint = c.lit(WHITE);
    c.world(sprite, at, Draw::tint(tint).angle(-a));
}

fn draw_unit_at<R: Renderer>(c: &mut Ctx<R>, u: &game::units::Unit, x: f32, y: f32) {
    let state = match u.anim() {
        Anim::Idle => "idle",
        Anim::Walk => "walk",
        Anim::Run => "run",
        Anim::Hammer => "hammer",
        Anim::Shoot => "shoot",
        Anim::Stunned => "stunned",
        Anim::Farm => "work",
        Anim::Slash => "slash",
        Anim::Push => "push",
    };
    // orig: Archer.ConvertToSoldier swaps in the soldier animator.
    let prefix = if u.soldier {
        "archer_soldier"
    } else {
        role_clip(u.role)
    };
    let Some(sprite) = c.frame(&format!("{prefix}_{state}"), u.anim_t) else {
        return;
    };
    // orig: Character: SpriteFX.recolorA = outfit, recolorB = skin.
    let d = Draw::tint(c.lit(WHITE))
        .flip(u.mover.facing < 0)
        .recolor(Color::rgb(u.outfit), Color::rgb(u.skin));
    c.r.sprite(sprite, world_to_screen(c.s, x, y), d);
}

fn draw_troll<R: Renderer>(c: &mut Ctx<R>, i: usize) {
    use game::greed::EnemyKind;
    let s = c.s;
    let t = &s.game.greed.trolls[i];
    let clip = match (t.kind, t.state) {
        (EnemyKind::Squid, TrollState::Dead { .. }) => "squid_death",
        (EnemyKind::Squid, _) => "squid_move",
        (EnemyKind::Boss | EnemyKind::KillerBoss, TrollState::Dead { .. }) => "boss_death",
        (EnemyKind::Boss | EnemyKind::KillerBoss, _) if t.bash.is_some() => "boss_bash_right",
        (EnemyKind::Boss | EnemyKind::KillerBoss, _) if t.puke.is_some() => "boss_puke",
        (EnemyKind::Boss | EnemyKind::KillerBoss, _) if t.mover.vx.abs() < 0.05 => "boss_idle",
        (EnemyKind::Boss | EnemyKind::KillerBoss, _) => "boss_walk",
        (_, st) => match st {
            TrollState::Dead { .. } => "troll_die",
            TrollState::Charge { landed: true, .. } => "troll_recover",
            TrollState::Charge { .. } => "troll_charge",
            _ if t.mover.vx.abs() < 0.05 => "troll_stand",
            _ if t.loot.is_some() => "troll_walk_hold",
            _ => "troll_walk",
        },
    };
    let Some(sprite) = c.frame(clip, t.anim_t) else {
        return;
    };
    let mut tint = c.lit(WHITE);
    if let TrollState::Dead { t: dt } = t.state {
        tint.a = (1.0 - (dt - 0.5).max(0.0)).max(0.0);
    }
    c.r.sprite(
        sprite,
        world_to_screen(s, t.x, t.y),
        Draw::tint(tint).flip(t.mover.facing < 0),
    );
    // What it carries. orig: Troll.PickupLoot puts it at (0.1875, 0.25) with x mirrored.
    if let Some(loot) = t.loot {
        let id = match loot {
            game::greed::Loot::Coin => c.sheet.id("coin_spin_0"),
            game::greed::Loot::Crown => c.sheet.id("crown"),
            game::greed::Loot::BoatPart(_) => c.sheet.id("boat_skeleton_s052_124"),
            game::greed::Loot::Tool(k) => s
                .world
                .tool_sprites
                .get(k as usize)
                .copied()
                .flatten()
                .and_then(|w| c.sheet.world(w)),
        };
        if let Some(id) = id {
            let f = t.mover.facing as f32;
            let at = world_to_screen(s, t.x + 0.1875 * f, t.y + 0.25);
            c.r.sprite(id, at, Draw::tint(tint).flip(t.mover.facing > 0));
        }
    }
}

fn draw_arrow<R: Renderer>(c: &mut Ctx<R>, i: usize) {
    let a = &c.s.game.greed.arrows[i];
    // orig: Arrow.Update points the arrow along its velocity.
    let angle = (-a.vy).atan2(a.vx);
    let at = world_to_screen(c.s, a.x, a.y);
    let tint = c.lit(WHITE);
    c.named("arrow", at, Draw::tint(tint).angle(angle));
}

fn draw_crown<R: Renderer>(c: &mut Ctx<R>) {
    let Some(cr) = &c.s.game.greed.crown else {
        return;
    };
    let at = world_to_screen(c.s, cr.x, cr.y);
    c.named("crown", at, Draw::tint(WHITE));
}

/// orig: `Grass.stage` setter: `grassSprites[stage * 8 + variation]`.
fn draw_grass<R: Renderer>(c: &mut Ctx<R>, i: usize) {
    let s = c.s;
    let Some(g) = &s.world.grass else { return };
    let p = &s.game.meadows.patches[i];
    let Some(&sprite) = g.sprites.get(p.stage as usize * 8 + p.variation as usize) else {
        return;
    };
    let at = layer_to_screen(s, p.x, s.game.meadows.y, 1.0);
    let tint = c.lit(WHITE);
    c.world(sprite, at, Draw::tint(tint));
}

fn draw_animal<R: Renderer>(c: &mut Ctx<R>, i: usize) {
    use game::wildlife::{AnimalState, Species};
    let a = &c.s.game.wildlife.animals[i];
    let speed = a.mover.vx.abs();
    let clip = match (a.species, a.state) {
        (Species::Deer, AnimalState::Dead { .. }) => "deer_die",
        (Species::Deer, AnimalState::Eat { .. }) => "deer_eat",
        (Species::Deer, _) if speed > 1.5 => "deer_run",
        (Species::Deer, _) if speed > 0.05 => "deer_walk",
        (Species::Deer, _) => "deer_look",
        (Species::Bunny, AnimalState::Dead { .. }) => "bunny_dead",
        (Species::Bunny, _) if speed > 0.05 => "bunny_walk",
        (Species::Bunny, _) => "bunny_stand",
    };
    let Some(sprite) = c.frame(clip, a.anim_t) else {
        return;
    };
    let at = world_to_screen(c.s, a.x, 0.0);
    let tint = c.lit(WHITE);
    c.r.sprite(sprite, at, Draw::tint(tint).flip(a.mover.facing < 0));
}

fn draw_tool<R: Renderer>(c: &mut Ctx<R>, i: usize) {
    let s = c.s;
    let t = &s.game.population.tools[i];
    let Some(Some(sprite)) = s.world.tool_sprites.get(t.kind as usize) else {
        return;
    };
    let at = world_to_screen(s, t.x, t.y);
    // orig: PayableShield.MakeAvailable gives the shield its banner's colours.
    let shop = t
        .shop
        .and_then(|id| s.game.population.shields.iter().find(|sh| sh.object == id));
    let mut d = Draw::tint(c.lit(WHITE));
    if let Some(sh) = shop {
        d = d.recolor(Color::rgb(sh.colors.0), Color::rgb(sh.colors.1));
    }
    c.world(*sprite, at, d);
}

fn draw_coin<R: Renderer>(c: &mut Ctx<R>, i: usize) {
    let s = c.s;
    let coin = &s.game.coins.list[i];
    let at = world_to_screen(s, coin.x, coin.y);
    // Coins in flight to a payable or the purse don't spin (orig: pauseAnimationWhenFake).
    let sprite = match coin.motion {
        Motion::Free => c.frame("coin_spin", s.t),
        _ => None,
    }
    .or(c.sheet.id("coin_spin_0"));
    if let Some(id) = sprite {
        let tint = c.lit(Color::new(1.0, 1.0, 1.0, coin.alpha));
        c.r.sprite(id, at, Draw::tint(tint));
    }
}

fn draw_monarch<R: Renderer>(c: &mut Ctx<R>) {
    let s = c.s;
    let tint = c.lit(WHITE);
    // Steed sprite at the root, rider on the "King" child, breath puff on "Puff".
    let m = &s.game.monarch;
    let root = world_to_screen(s, m.x, s.game.monarch_y);
    let flip = m.facing < 0;
    let f = m.facing as f32;
    let steed = s.game.steed;
    let sheet = c.sheet;
    let Some(clip) = sheet.clip(&clip_name(&s.world.steeds[steed as usize].clips, s.anim.0)) else {
        return;
    };
    let t = s.anim.1;
    let child = |path: &str| {
        let (ox, oy) = clip.offset(path, t);
        offset(root, ox * f, oy)
    };
    if let Some(sp) = clip.sprite("", t) {
        let (mane, coat) = steed_colors(s, steed, true);
        c.r.sprite(sp, root, Draw::tint(tint).flip(flip).recolor(mane, coat));
    }
    // Clips that don't animate the rider (idle, eat) leave the prefab defaults.
    let who = heraldry::rider(s.game.look);
    let rider = clip.sprite(who.track, t).or(sheet.id(who.default));
    let king = child(who.track);
    if let Some(rider) = rider {
        let d = Draw::tint(tint)
            .flip(flip)
            .recolor(Color::rgb(s.game.coat.primary), SKIN);
        c.r.sprite(rider, king, d);
    }
    // Crown is a child of the rider, so its keys are relative to them.
    let (cx, cy) = if clip.has_offsets(who.crown_track) {
        clip.offset(who.crown_track, t)
    } else {
        CROWN_DEFAULT_POS
    };
    if s.game.has_crown {
        c.named(
            who.crown,
            offset(king, cx * f, cy),
            Draw::tint(tint).flip(flip),
        );
    }
    if m.is_puffing() {
        if let Some(puff) = c.frame("horse_puff", s.t) {
            c.r.sprite(puff, child("Puff"), Draw::tint(WHITE).flip(flip));
        }
    }
}

fn celestial_screen(s: &Session, i: usize) -> (f32, f32) {
    let c = &s.world.celestials[i];
    let (x, y) = celestial_position(c, s.game.clock.time);
    // Heaven has parallax ratio 0 in x: fixed to the camera.
    layer_to_screen(s, x, y, 0.0)
}

fn draw_celestial<R: Renderer>(c: &mut Ctx<R>, i: usize, glow: bool) {
    let s = c.s;
    let cel = &s.world.celestials[i];
    let light = c.light();
    let at = celestial_screen(s, i);
    if glow {
        let tint = Color::rgba(
            light.colors[if cel.is_sun {
                color::SUN_GLOW
            } else {
                color::MOON_GLOW
            }],
        );
        if let Some(g) = cel.glow {
            c.world(
                g,
                at,
                Draw::tint(tint).scale(cel.glow_scale).additive().smooth(),
            );
        }
    } else {
        let tint = Color::rgba(
            light.colors[if cel.is_sun {
                color::SUN_TINT
            } else {
                color::MOON_TINT
            }],
        );
        if let Some(sp) = cel.sprite {
            c.world(sp, at, Draw::tint(tint));
        }
    }
}

fn draw_dog<R: Renderer>(c: &mut Ctx<R>) {
    // orig: Dog SpriteFX recolorB = its colour.
    let Some(d) = &c.s.game.dogs.dog else { return };
    let speed = d.mover.vx.abs();
    let clip = if d.barking && speed < 0.1 {
        "dog_bark"
    } else if speed > 0.1 {
        "dog_run"
    } else {
        "dog_idle"
    };
    if let Some(sp) = c.frame(clip, d.anim_t) {
        let dr = Draw::tint(c.lit(WHITE))
            .flip(d.mover.facing < 0)
            .recolor(WHITE, Color::rgb(d.color));
        c.r.sprite(sp, world_to_screen(c.s, d.x, 0.0), dr);
    }
}

/// Everything on the top screen.
pub fn draw(s: &Session, sheet: &Sheet, r: &mut impl Renderer) {
    let mut c = Ctx { s, sheet, r };
    draw_world(&mut c);
    draw_overlays(&mut c);
}

fn draw_world<R: Renderer>(c: &mut Ctx<R>) {
    let s = c.s;
    let colors = s.game.blender.state.colors;
    let sky_top = Color::rgba(colors[color::SKY_TOP]);
    let sky_bottom = Color::rgba(colors[color::SKY_BOTTOM]);
    // Above the sky quad the original shows its top color; below the horizon,
    // the water reflects the sky (approximated until the reflection pass).
    c.r.depth(1.0);
    c.r.clear(sky_top);
    gradient(c.r, SKY.1, SKY.2, sky_bottom, sky_top);
    let below = screen_y(SKY.1);
    c.r.rect(Rect::new(0.0, below, W, H - below), sky_bottom);

    let game_z = layer_z(&s.world);
    for (key, item) in draw_list(s) {
        c.r.depth(stereo_depth(-key.2 - game_z));
        match item {
            DrawItem::Part(oi, pi) => draw_part(c, &s.game.level.objects[oi], pi),
            DrawItem::Scenery(oi, pi) => draw_part(c, &s.game.level.scenery[oi], pi),
            DrawItem::Scaffold(oi, mi) => draw_scaffold(c, oi, mi),
            DrawItem::PriceSlots => draw_price_slots(c),
            DrawItem::Unit(i) => draw_unit(c, i),
            DrawItem::Tool(i) => draw_tool(c, i),
            DrawItem::Troll(i) => draw_troll(c, i),
            DrawItem::Arrow(i) => draw_arrow(c, i),
            DrawItem::Crown => draw_crown(c),
            DrawItem::Coin(i) => draw_coin(c, i),
            DrawItem::Monarch => draw_monarch(c),
            DrawItem::Fill => gradient(
                c.r,
                FILL.1,
                FILL.2,
                Color::rgba(colors[color::FILL_BOTTOM]),
                Color::rgba(colors[color::FILL_TOP]),
            ),
            DrawItem::HazeFar | DrawItem::HazeNear => {
                let (_, b, t) = if matches!(item, DrawItem::HazeFar) {
                    HAZE_FAR
                } else {
                    HAZE_NEAR
                };
                let haze = Color::rgba(colors[color::HAZE]);
                if haze.a > 0.0 {
                    gradient(c.r, b, t, haze, Color { a: 0.0, ..haze });
                }
            }
            DrawItem::Water => draw_water(c),
            DrawItem::Celestial(i) => draw_celestial(c, i, false),
            DrawItem::Glow(i) => draw_celestial(c, i, true),
            DrawItem::Grass(i) => draw_grass(c, i),
            DrawItem::Animal(i) => draw_animal(c, i),
            DrawItem::FreeSteed(i) => draw_free_steed(c, i),
            DrawItem::Hermit(i) => draw_hermit(c, i),
            DrawItem::Bolt(i) => draw_bolt(c, i),
            DrawItem::Merchant(i) => draw_merchant(c, i, false),
            DrawItem::Mule(i) => draw_merchant(c, i, true),
            DrawItem::Chest(i) => draw_chest(c, i),
            DrawItem::Banker => draw_banker(c),
            DrawItem::Dog => draw_dog(c),
            DrawItem::LoosePart(i) => draw_loose_part(c, i),
            DrawItem::Deckhand(i) => draw_deckhand(c, i),
            DrawItem::Fire { o, f, scenery } => {
                let objs = if scenery { &s.game.level.scenery } else { &s.game.level.objects };
                let o = &objs[o];
                let fire = s.world.protos[o.proto as usize].fires[f];
                let dir = if o.flip { -1.0 } else { 1.0 };
                let at = layer_to_screen(s, o.x + dir * fire.at.0, o.y + fire.at.1, s.world.layer(o.layer).ratio);
                draw_fire(c, at, &fire, o.id.wrapping_mul(7) + f as u32);
            }
            DrawItem::Torch => {
                let m = &s.game.monarch;
                if let Some(mut fire) = s.world.steeds[s.game.steed as usize].torch {
                    fire.real_light = true;
                    let f = m.facing as f32;
                    let at = world_to_screen(s, m.x + fire.at.0 * f, fire.at.1);
                    draw_fire(c, at, &fire, 1);
                }
            }
            DrawItem::Boulder(i) => {
                let b = &s.game.siege.boulders[i];
                if let Some(sp) = s.world.boulder_sprite {
                    let at = world_to_screen(s, b.x, b.y);
                    let tint = c.lit(WHITE);
                    c.world(sp, at, Draw::tint(tint).angle(b.spin.to_radians()));
                }
            }
        }
    }

    c.r.depth(0.0);
    // The teleporter exit (placeholder rings until its sprites are drawn).
    if let Some(e) = s.game.teleports.exit {
        let at = world_to_screen(s, e.x, 0.6);
        for radius in [6.0, 10.0, 14.0] {
            c.r.circle_lines(at.0, at.1, radius, 1.0, Color::new(0.7, 0.9, 1.0, 0.8));
        }
    }

    // orig: PostFX adds `additiveColor` over the whole frame.
    let add = colors[color::ADDITIVE];
    if add[0] + add[1] + add[2] > 0.002 {
        c.r.add(
            Rect::new(0.0, 0.0, W, H),
            Color::new(add[0], add[1], add[2], 1.0),
        );
    }
}

/// The water: its colour, and over it last frame's scene above the horizon,
/// mirrored, squashed and rippled row by row. Opaque, so whatever has set
/// below the horizon stays hidden. orig: `Water` with `Custom/Water`, which
/// displaces each pixel; here each row moves as a whole (no fragment shaders
/// on the 3DS).
fn draw_water<R: Renderer>(c: &mut Ctx<R>) {
    let s = c.s;
    let w = &s.world.water;
    let colors = s.game.blender.state.colors;
    let water = Color::rgba(colors[color::WATER]);
    let horizon = screen_y(w.top);
    c.r.rect(Rect::new(0.0, horizon, W, H - horizon), Color { a: 1.0, ..water });
    // orig: _MaxDisplacement z/w weigh the coarse and fine maps (dry weather).
    let (coarse_k, fine_k) = (0.3, 0.2);
    let (mw, mh) = (w.size.0 as f32, w.size.1 as f32);
    // Texel column under the screen's centre: the band is `repeat.0` maps wide.
    let band_px = mw * w.repeat.0;
    let centre = band_px / 2.0;
    let bottom = screen_y(w.bottom).min(H);
    let squash = w.reflection_scale.max(0.1);
    // Lifted a third of the way to white: on the 3DS screen the plain
    // multiply read too dark against open ground (forest water stays dark,
    // since it mirrors the forest).
    let lift = |v: f32| v + (1.0 - v) * 0.35;
    let tint = Color::new(lift(water.r), lift(water.g), lift(water.b), water.a);
    let mut y = horizon;
    while y < bottom {
        let row = y - horizon;
        let ty = -(row * mh / (bottom - horizon).max(1.0));
        let a = w.sample(&w.coarse, centre + s.water_scroll.0, ty);
        let b = w.sample(&w.fine, centre + s.water_scroll.1, ty);
        let dx = ((a.0 * coarse_k + b.0 * fine_k) * w.max_displacement.0).round();
        let dy = (a.1 * coarse_k + b.1 * fine_k) * w.max_displacement.1;
        // The row this one mirrors, `1 / squash` rows tall, kept on screen.
        let src_y = (horizon - row / squash + dy).clamp(1.0 / squash, horizon);
        let (sx, dx0) = if dx >= 0.0 { (dx, 0.0) } else { (0.0, -dx) };
        let span = W - dx.abs();
        c.r.reflect(
            Rect::new(dx0, y, span, 1.0),
            Rect::new(sx, src_y, span, -1.0 / squash),
            tint,
        );
        y += 1.0;
    }
}

fn centered<R: Renderer>(c: &mut Ctx<R>, text: &str, y: f32, size: f32) {
    let (w, _) = crate::text::measure(c.sheet, text, size);
    crate::text::draw(c.r, c.sheet, text, ((W - w) / 2.0).round(), y, size, WHITE);
}

/// Bottom-right: a spinning coin while saving, then the result, fading out.
fn draw_save_notice<R: Renderer>(c: &mut Ctx<R>) {
    use crate::session::SaveNotice;
    let Some(notice) = c.s.save_notice else { return };
    let (text, spin, left) = match notice {
        SaveNotice::Saving(..) => ("Saving", true, 1.0),
        SaveNotice::Saved(t) => ("Game saved", false, t),
        SaveNotice::Failed(t) => ("Save failed", false, t),
    };
    let tint = Color::new(1.0, 1.0, 1.0, (left / 0.5).min(1.0));
    let (w, _) = crate::text::measure(c.sheet, text, 14.0);
    let (x, y) = (W - 8.0, H - 8.0);
    let coin = if spin {
        c.frame("coin_spin", c.s.t * 2.0)
    } else {
        c.sheet.id("coin_spin_0")
    };
    if matches!(notice, SaveNotice::Failed(_)) {
        crate::text::draw(c.r, c.sheet, text, x - w, y, 14.0, tint);
        return;
    }
    if let Some(id) = coin {
        c.r.sprite(id, (x - 4.0, y - 5.0), Draw::tint(tint));
    }
    crate::text::draw(c.r, c.sheet, text, x - 14.0 - w, y, 14.0, tint);
}

fn draw_overlays<R: Renderer>(c: &mut Ctx<R>) {
    let s = c.s;
    let g = &s.game;
    let m = &g.monarch;
    if g.game_over || (g.won && s.won_for > 4.0) {
        let text = if g.won && g.harbor.boat.is_some() {
            "The monarch sets sail"
        } else if g.won {
            "The cliff portals have fallen"
        } else {
            "The greed took the crown"
        };
        centered(c, text, 90.0, 24.0);
        let hint = if g.won {
            "Pick an island to sail to"
        } else {
            "Pick an island to start again"
        };
        centered(c, hint, 110.0, 16.0);
    }
    if let Some((day, _)) = s.day_banner {
        centered(c, &format!("Day {day}"), 60.0, 32.0);
    }
    draw_save_notice(c);

    if s.debug {
        let lines = [
            format!(
                "D{} {:02}:{:02}  x{}",
                g.clock.day,
                g.clock.time as u32,
                (g.clock.time.fract() * 60.0) as u32,
                s.speed
            ),
            format!("x {:.2}  v {:.2}  {:?}", m.x, m.velocity, m.action()),
            format!(
                "stamina {:.2}  tired {:.1}  fed {:.1}",
                m.stamina,
                m.tired_timer.max(0.0),
                m.well_fed_timer.max(0.0)
            ),
            format!("pay {:?}  floating {}", g.pay.state(), g.pay.floating()),
            format!("coins on ground {}", g.coins.list.len()),
            format!(
                "beggars {} peasants {} workers {} archers {}  border {:.0}..{:.0} {}",
                g.population.count(Role::Beggar),
                g.population.count(Role::Peasant),
                g.population.count(Role::Worker),
                g.population.count(Role::Archer),
                g.borders.left,
                g.borders.right,
                if g.safe { "safe" } else { "unsafe" }
            ),
            format!(
                "island {} {:.0}..{:.0}  objects {}",
                s.world.levels[g.level_index].name,
                g.level.left,
                g.level.left + g.level.width,
                g.level.objects.len()
            ),
        ];
        let note = s.note.lines().map(str::to_owned);
        let shown = if s.debug_lines { lines.len() } else { 0 };
        for (i, l) in lines.into_iter().take(shown).chain(note).enumerate() {
            crate::text::draw(c.r, c.sheet, &l, 4.0, 12.0 + i as f32 * 11.0, 14.0, WHITE);
        }
    }
}
