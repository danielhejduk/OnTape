//! Drawing the cassette.
//!
//! Everything is measured in millimetres of a real compact cassette
//! (100.4 × 63.8 mm) and scaled to fit the widget. The tape packs use the
//! reel radii from the core's reel physics, so they really grow and shrink,
//! and the hubs turn by exactly as many turns as the tape on them.
//!
//! GTK rasterises a cairo drawing on the CPU and uploads it to the GPU again
//! every time it repaints it, which is far too much work for a picture that
//! moves. So cairo draws only what stays the same from frame to frame, into
//! [`Textures`]: the shell, a cover with the window cut out of it, and the
//! parts of a hub. [`scene`] then puts each frame together from those
//! textures and two gradients, as render nodes that the GPU draws by itself:
//! the packs go between the shell and the cover, and of a hub only the teeth
//! and the notch move.
//!
//! What GTK repaints of a frame is the rectangles around the nodes that
//! changed, with everything under and over them. That is why the moving
//! pieces are kept as small as they can be.

use std::f64::consts::TAU;

use ontape_core::deck::TapeStatus;
use ontape_core::tape::{Reels, Side};
use relm4::gtk::cairo::{self, Context, FontSlant, FontWeight, LinearGradient};
use relm4::gtk::prelude::*;
use relm4::gtk::{gdk, glib, graphene, gsk};

use crate::transport::side_name;

/// Size of a compact cassette, in millimetres.
const WIDTH: f64 = 100.4;
const HEIGHT: f64 = 63.8;
/// Space around the cassette inside the widget, in pixels.
const MARGIN: f64 = 12.0;

const LEFT_HUB: (f64, f64) = (28.95, 29.0);
const RIGHT_HUB: (f64, f64) = (71.45, 29.0);
/// Radius of an empty hub, mm (the core's `HUB_RADIUS` is 11 mm too).
const HUB: f64 = 11.0;
/// Radius of the hub as drawn: a little less, so a thin pack shows around it.
const HUB_DISC: f64 = HUB - 0.2;
/// How far from the hub's centre its teeth (and their blur) reach.
const TEETH_REACH: f64 = 6.5;
/// The notch on the hub's rim: its radius, and how far from the centre it is.
const NOTCH_RADIUS: f64 = 0.9;
const NOTCH_ORBIT: f64 = 8.6;
/// The clear window between the hubs: x, y, width, height.
const WINDOW: (f64, f64, f64, f64) = (23.0, 19.0, 54.4, 20.0);
/// The round openings in the label around each hub.
const HUB_OPENING: f64 = 12.4;
/// A tape pack is drawn with its radius rounded to this fraction of a pixel.
const PACK_STEP: f64 = 0.125;
/// Above this tape speed the hub teeth blur into a ring.
const BLUR_SPEED: f64 = 8.0;

/// A moving tape is redrawn at most this often, in microseconds. The hubs
/// turn well under once a second at playing speed, so 30 frames a second
/// look as smooth as the monitor's full rate and cost half as much or less.
const FRAME_INTERVAL: i64 = 1_000_000 / 30;
/// Frame clocks tick a little early or late; without this a 60 Hz clock
/// would get every third frame instead of every second one.
const FRAME_SLACK: i64 = 2_000;

/// Whether a moving tape should be redrawn on the frame at `now`, given
/// the frame time it was last redrawn at. Both are frame-clock times in
/// microseconds.
pub fn frame_due(last: Option<i64>, now: i64) -> bool {
    last.is_none_or(|last| now - last >= FRAME_INTERVAL - FRAME_SLACK)
}

/// What the drawing needs to know about the inserted tape.
#[derive(Clone, Debug, PartialEq)]
pub struct CassetteView {
    pub label: Label,
    pub reels: ReelsView,
}

/// What is written on the cassette. It changes only when the tape does.
#[derive(Clone, Debug, PartialEq)]
pub struct Label {
    pub title: String,
    pub side: Side,
    pub cassette: String,
}

/// The moving parts behind the window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReelsView {
    /// Tape pack radii (left, right), mm.
    pub radii: (f64, f64),
    /// Hub rotation (left, right), radians.
    pub angles: (f64, f64),
    /// Tape speed as a multiple of playback speed.
    pub speed: f64,
}

impl CassetteView {
    pub fn new(title: String, status: &TapeStatus) -> CassetteView {
        CassetteView {
            label: Label { title, side: status.facing, cassette: status.cassette.to_string() },
            reels: ReelsView {
                radii: (status.reel_radii.0 * 1000.0, status.reel_radii.1 * 1000.0),
                angles: reel_angles(status),
                speed: status.speed,
            },
        }
    }
}

/// How far each hub has turned, worked out from the tape on it rather than
/// added up frame by frame: the right reel holds what has been played, the
/// left reel the rest. Both turn the same way as the tape moves forward.
pub fn reel_angles(status: &TapeStatus) -> (f64, f64) {
    let reels = Reels::for_cassette(status.cassette);
    let left = -TAU * reels.reel_turns(status.side_len.saturating_sub(status.position));
    let right = TAU * reels.reel_turns(status.position);
    (left, right)
}

/// Where the cassette sits in a widget, in pixels.
struct Placement {
    /// Pixels per millimetre.
    scale: f64,
    /// The cassette's top-left corner.
    x: f64,
    y: f64,
}

impl Placement {
    /// Makes `cr`, which draws in the widget's pixels, draw in the
    /// cassette's millimetres.
    fn apply(&self, cr: &Context) {
        cr.translate(self.x, self.y);
        cr.scale(self.scale, self.scale);
    }

    /// The pixels (x, y, width, height) around the window and both hub
    /// openings: everything that moves stays inside them.
    fn window(&self) -> (i32, i32, i32, i32) {
        let (wx, wy, ww, wh) = WINDOW;
        let left = wx.min(LEFT_HUB.0 - HUB_OPENING);
        let right = (wx + ww).max(RIGHT_HUB.0 + HUB_OPENING);
        let top = wy.min(LEFT_HUB.1.min(RIGHT_HUB.1) - HUB_OPENING);
        let bottom = (wy + wh).max(LEFT_HUB.1.max(RIGHT_HUB.1) + HUB_OPENING);
        // A pixel more all round for the antialiased edges.
        let x0 = (self.x + left * self.scale).floor() as i32 - 1;
        let y0 = (self.y + top * self.scale).floor() as i32 - 1;
        let x1 = (self.x + right * self.scale).ceil() as i32 + 1;
        let y1 = (self.y + bottom * self.scale).ceil() as i32 + 1;
        (x0, y0, x1 - x0, y1 - y0)
    }
}

/// Fits the cassette into a `width` × `height` widget, if there is room.
fn placement(width: f64, height: f64) -> Option<Placement> {
    let scale = ((width - 2.0 * MARGIN) / WIDTH).min((height - 2.0 * MARGIN) / HEIGHT);
    (scale > 0.0).then(|| Placement { scale, x: (width - WIDTH * scale) / 2.0, y: (height - HEIGHT * scale) / 2.0 })
}

/// Draws everything that stays put — the shell with its label, or an empty
/// deck when `label` is `None` — to fill a `width` × `height` widget. `ink`
/// is the theme's text colour.
fn draw_shell(cr: &Context, width: f64, height: f64, label: Option<&Label>, ink: (f64, f64, f64)) -> Result<(), cairo::Error> {
    let Some(at) = placement(width, height) else { return Ok(()) };
    at.apply(cr);
    match label {
        Some(label) => draw_cassette(cr, label),
        None => draw_empty_well(cr, ink),
    }
}

/// Everything that is drawn with cairo, as textures. They change only with
/// the widget's size and the tape's label.
pub struct Textures {
    /// The shell — or the empty deck — over the whole widget.
    shell: gdk::Texture,
    /// What a tape's moving picture is made of, when there is a tape.
    tape: Option<TapeTextures>,
    /// Device pixels per pixel.
    device_scale: i32,
}

struct TapeTextures {
    /// The label around the window, with the window cut out of it and a
    /// little glare on the glass. It lies over the tape packs, so that they
    /// show only through the window.
    cover: gdk::Texture,
    /// Where the cover lies in the widget.
    window: graphene::Rect,
    /// The parts of a hub, each with the rectangle it fills around its own
    /// centre: the disc, which stays put; the teeth, which turn; the soft
    /// ring shown instead of them when they spin too fast to see; and the
    /// notch, which goes round the rim.
    hub: Sprite,
    teeth: Sprite,
    blur: Sprite,
    notch: Sprite,
}

type Sprite = (gdk::Texture, graphene::Rect);

impl Textures {
    /// Draws the textures for a `width` × `height` widget with
    /// `device_scale` device pixels per pixel, showing a tape with `label`
    /// or an empty deck. `ink` is the theme's text colour. `None` only if
    /// cairo fails.
    pub fn draw(width: i32, height: i32, device_scale: i32, label: Option<&Label>, ink: (f64, f64, f64)) -> Option<Textures> {
        let (w, h) = (f64::from(width), f64::from(height));
        let shell = texture((0, 0, width, height), device_scale, |cr| draw_shell(cr, w, h, label, ink))?;
        let tape = match (label, placement(w, h)) {
            (Some(label), Some(at)) => Some(TapeTextures::draw(&at, device_scale, label)?),
            _ => None,
        };
        Some(Textures { shell, tape, device_scale })
    }
}

impl TapeTextures {
    fn draw(at: &Placement, device_scale: i32, label: &Label) -> Option<TapeTextures> {
        let window = at.window();
        let cover = texture(window, device_scale, |cr| {
            at.apply(cr);
            draw_cassette(cr, label)?;
            // Cut the window out, then put the glare on the glass.
            cr.set_operator(cairo::Operator::Clear);
            window_path(cr);
            cr.fill()?;
            cr.set_operator(cairo::Operator::Over);
            let (wx, wy, ww, wh) = WINDOW;
            let glare = LinearGradient::new(wx, wy, wx + ww, wy + wh);
            glare.add_color_stop_rgba(0.0, 1.0, 1.0, 1.0, 0.10);
            glare.add_color_stop_rgba(0.45, 1.0, 1.0, 1.0, 0.02);
            glare.add_color_stop_rgba(1.0, 1.0, 1.0, 1.0, 0.07);
            window_path(cr);
            cr.set_source(&glare)?;
            cr.fill()
        })?;

        // A sprite is a square around its own centre, with a pixel to spare
        // for the antialiased edges.
        let sprite = |radius: f64, part: HubPart| {
            let half = (radius * at.scale).ceil() as i32 + 1;
            let bounds = (-half, -half, 2 * half, 2 * half);
            let texture = texture(bounds, device_scale, |cr| {
                cr.scale(at.scale, at.scale);
                draw_hub(cr, part)
            })?;
            Some((texture, rect(bounds)))
        };
        Some(TapeTextures {
            cover,
            window: rect(window),
            hub: sprite(HUB_DISC, HubPart::Disc)?,
            teeth: sprite(TEETH_REACH, HubPart::Teeth)?,
            blur: sprite(TEETH_REACH, HubPart::Blur)?,
            notch: sprite(NOTCH_RADIUS, HubPart::Notch)?,
        })
    }
}

/// Lets cairo draw the part of a widget inside `bounds` (x, y, width,
/// height, in pixels) and makes a texture of it.
fn texture(
    (x, y, width, height): (i32, i32, i32, i32),
    device_scale: i32,
    draw: impl FnOnce(&Context) -> Result<(), cairo::Error>,
) -> Option<gdk::Texture> {
    let (device_width, device_height) = (width * device_scale, height * device_scale);
    let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, device_width, device_height).ok()?;
    surface.set_device_scale(f64::from(device_scale), f64::from(device_scale));
    {
        let cr = Context::new(&surface).ok()?;
        cr.translate(-f64::from(x), -f64::from(y));
        draw(&cr).ok()?;
    }
    // Cairo's ARGB32 is one native-endian word per pixel, alpha premultiplied.
    let format = if cfg!(target_endian = "little") {
        gdk::MemoryFormat::B8g8r8a8Premultiplied
    } else {
        gdk::MemoryFormat::A8r8g8b8Premultiplied
    };
    let stride = usize::try_from(surface.stride()).ok()?;
    let pixels = glib::Bytes::from(&*surface.data().ok()?);
    Some(gdk::MemoryTexture::new(device_width, device_height, format, &pixels, stride).upcast())
}

fn rect((x, y, width, height): (i32, i32, i32, i32)) -> graphene::Rect {
    graphene::Rect::new(x as f32, y as f32, width as f32, height as f32)
}

/// Puts the picture for a `width` × `height` widget together from its
/// `textures`: the shell alone for an empty deck, or with the tape packs
/// and hubs of `reels` showing through the window.
///
/// Each frame is a handful of textures and plain rings, with nothing to
/// rasterise, clip to a shape or draw off screen first.
pub fn scene(width: i32, height: i32, textures: &Textures, reels: Option<&ReelsView>) -> gsk::RenderNode {
    let mut layers = vec![gsk::TextureNode::new(&textures.shell, &rect((0, 0, width, height))).upcast()];
    if let (Some(tape), Some(reels), Some(at)) = (&textures.tape, reels, placement(f64::from(width), f64::from(height))) {
        // The packs, in the cassette's millimetres. They reach past the
        // window's outline; the cover hides that.
        let mut packs = Vec::new();
        for (centre, radius) in [(LEFT_HUB, reels.radii.0), (RIGHT_HUB, reels.radii.1)] {
            if radius > HUB + 0.05 {
                // A playing tape winds on a few thousandths of a pixel per
                // frame. Rounded to what can be seen, the pack stays the very
                // same node for a second or so and is not repainted.
                let step = PACK_STEP / at.scale;
                let device_pixel = 1.0 / (at.scale * f64::from(textures.device_scale));
                packs.push(pack(centre, (radius / step).round() * step, device_pixel));
            }
        }
        if !packs.is_empty() {
            let to_pixels = gsk::Transform::new().translate(&graphene::Point::new(at.x as f32, at.y as f32)).scale(at.scale as f32, at.scale as f32);
            let packs = gsk::TransformNode::new(gsk::ContainerNode::new(&packs), Some(&to_pixels));
            layers.push(gsk::ClipNode::new(packs, &tape.window).upcast());
        }

        let blurred = reels.speed > BLUR_SPEED;
        for ((x, y), angle) in [(LEFT_HUB, reels.angles.0), (RIGHT_HUB, reels.angles.1)] {
            // Puts a sprite on the hub, `out` millimetres from its centre
            // towards where the notch is.
            let (sin, cos) = angle.sin_cos();
            let place = |(texture, bounds): &Sprite, out: f64, turn: bool| {
                let centre = graphene::Point::new((at.x + (x + out * sin) * at.scale) as f32, (at.y + (y - out * cos) * at.scale) as f32);
                let mut onto_hub = gsk::Transform::new().translate(&centre);
                if turn {
                    // The angle counts every turn since the start of the
                    // tape; only what is left of the last one shows.
                    onto_hub = onto_hub.rotate(angle.rem_euclid(TAU).to_degrees() as f32);
                }
                gsk::TransformNode::new(gsk::TextureNode::new(texture, bounds), Some(&onto_hub)).upcast()
            };
            layers.push(place(&tape.hub, 0.0, false));
            if blurred {
                layers.push(place(&tape.blur, 0.0, false));
            } else {
                layers.push(place(&tape.teeth, 0.0, true));
                layers.push(place(&tape.notch, NOTCH_ORBIT, false));
            }
        }
        layers.push(gsk::TextureNode::new(&tape.cover, &tape.window).upcast());
    }
    gsk::ContainerNode::new(&layers).upcast()
}

/// A tape pack: a brown disc with a lighter 0.3 mm line along its rim.
/// `pixel` is the size of a device pixel in millimetres.
///
/// It is one radial gradient with hard steps, which costs the GPU far less
/// than a disc and a ring cut out with rounded outlines. Each step is spread
/// over one pixel, to keep the edges smooth.
fn pack((x, y): (f64, f64), radius: f64, pixel: f64) -> gsk::RenderNode {
    let wound = gdk::RGBA::new(0.29, 0.18, 0.12, 1.0);
    let rim = gdk::RGBA::new(0.42, 0.28, 0.19, 1.0);
    let (inside, outside) = (radius - 0.15, radius + 0.15);
    let end = outside + pixel / 2.0;
    let stop = |at: f64, colour| gsk::ColorStop::new((at / end) as f32, colour);
    let stops = [
        stop(0.0, wound),
        stop(inside - pixel / 2.0, wound),
        stop(inside + pixel / 2.0, rim),
        stop(outside - pixel / 2.0, rim),
        stop(end, rim.with_alpha(0.0)),
    ];
    let (x, y, end) = (x as f32, y as f32, end as f32);
    let bounds = graphene::Rect::new(x - end, y - end, 2.0 * end, 2.0 * end);
    gsk::RadialGradientNode::new(&bounds, &graphene::Point::new(x, y), end, end, 0.0, 1.0, &stops).upcast()
}

fn draw_cassette(cr: &Context, label: &Label) -> Result<(), cairo::Error> {
    // Shadow and shell.
    rounded_rect(cr, 0.6, 1.0, WIDTH, HEIGHT, 3.2);
    cr.set_source_rgba(0.0, 0.0, 0.0, 0.25);
    cr.fill()?;
    rounded_rect(cr, 0.0, 0.0, WIDTH, HEIGHT, 3.2);
    cr.set_source_rgb(0.18, 0.18, 0.2);
    cr.fill_preserve()?;
    cr.set_source_rgb(0.29, 0.29, 0.32);
    cr.set_line_width(0.4);
    cr.stroke()?;

    // Screws.
    for (x, y) in [(4.2, 4.2), (WIDTH - 4.2, 4.2), (4.2, HEIGHT - 4.2), (WIDTH - 4.2, HEIGHT - 4.2), (WIDTH / 2.0, 58.0)] {
        cr.arc(x, y, 1.3, 0.0, TAU);
        cr.set_source_rgb(0.55, 0.55, 0.57);
        cr.fill()?;
        cr.move_to(x - 0.8, y);
        cr.line_to(x + 0.8, y);
        cr.set_source_rgb(0.3, 0.3, 0.32);
        cr.set_line_width(0.3);
        cr.stroke()?;
    }

    // Label with its stripes.
    rounded_rect(cr, 5.0, 3.5, WIDTH - 10.0, 42.0, 2.0);
    cr.set_source_rgb(0.95, 0.91, 0.82);
    cr.fill()?;
    cr.rectangle(5.0, 13.2, WIDTH - 10.0, 2.4);
    cr.set_source_rgb(0.91, 0.52, 0.23);
    cr.fill()?;
    cr.rectangle(5.0, 15.6, WIDTH - 10.0, 1.2);
    cr.set_source_rgb(0.79, 0.27, 0.17);
    cr.fill()?;

    // Title, side and cassette type, in pen-like dark ink.
    cr.set_source_rgb(0.15, 0.16, 0.3);
    cr.select_font_face("Sans", FontSlant::Normal, FontWeight::Bold);
    cr.set_font_size(4.4);
    let title = fit_text(cr, &label.title, WIDTH - 22.0)?;
    cr.move_to(10.0, 10.6);
    cr.show_text(&title)?;
    cr.set_font_size(9.0);
    centered_text(cr, side_name(label.side), 11.0, 32.5)?;
    cr.set_font_size(4.2);
    centered_text(cr, &label.cassette, 89.7, 31.0)?;

    // The dark inside of the window; the packs and hubs go over it.
    window_path(cr);
    cr.set_source_rgb(0.08, 0.08, 0.09);
    cr.fill()?;

    // Head opening along the bottom edge.
    cr.move_to(20.0, HEIGHT);
    cr.line_to(24.5, 51.0);
    cr.line_to(WIDTH - 24.5, 51.0);
    cr.line_to(WIDTH - 20.0, HEIGHT);
    cr.close_path();
    cr.set_source_rgb(0.24, 0.24, 0.27);
    cr.fill()?;
    cr.set_source_rgb(0.07, 0.07, 0.08);
    for x in [28.0, WIDTH - 28.0] {
        cr.arc(x, 58.5, 1.6, 0.0, TAU);
        cr.fill()?;
    }
    rounded_rect(cr, 44.2, 54.5, 12.0, 5.0, 0.8);
    cr.fill()?;
    Ok(())
}

/// The see-through part of the label: the window between the hubs plus a
/// round opening around each hub. All parts wind the same way, so filling
/// the path covers their union.
fn window_path(cr: &Context) {
    let (wx, wy, ww, wh) = WINDOW;
    rounded_rect(cr, wx, wy, ww, wh, 2.0);
    for (cx, cy) in [LEFT_HUB, RIGHT_HUB] {
        cr.new_sub_path();
        cr.arc(cx, cy, HUB_OPENING, 0.0, TAU);
        cr.close_path();
    }
}

/// The parts a hub is drawn in, each around the origin.
#[derive(Clone, Copy)]
enum HubPart {
    /// The white disc with its dark hole.
    Disc,
    /// The six drive teeth reaching into the hole.
    Teeth,
    /// What the teeth look like spinning too fast to see: a soft ring.
    Blur,
    /// The notch on the rim that shows the hub turning. It is drawn at the
    /// origin and belongs [`NOTCH_ORBIT`] away from the hub's centre.
    Notch,
}

fn draw_hub(cr: &Context, part: HubPart) -> Result<(), cairo::Error> {
    match part {
        HubPart::Disc => {
            cr.arc(0.0, 0.0, HUB_DISC, 0.0, TAU);
            cr.set_source_rgb(0.86, 0.86, 0.84);
            cr.fill()?;
            cr.arc(0.0, 0.0, 6.2, 0.0, TAU);
            cr.set_source_rgb(0.08, 0.08, 0.09);
            cr.fill()
        }
        HubPart::Teeth => {
            cr.set_source_rgb(0.86, 0.86, 0.84);
            for i in 0..6 {
                cr.save()?;
                cr.rotate(f64::from(i) * TAU / 6.0);
                cr.rectangle(-0.7, 4.4, 1.4, 2.0);
                cr.fill()?;
                cr.restore()?;
            }
            Ok(())
        }
        HubPart::Blur => {
            cr.arc(0.0, 0.0, 5.4, 0.0, TAU);
            cr.set_source_rgba(0.86, 0.86, 0.84, 0.35);
            cr.set_line_width(1.6);
            cr.stroke()
        }
        HubPart::Notch => {
            cr.arc(0.0, 0.0, NOTCH_RADIUS, 0.0, TAU);
            cr.set_source_rgb(0.45, 0.45, 0.47);
            cr.fill()
        }
    }
}

fn draw_empty_well(cr: &Context, (r, g, b): (f64, f64, f64)) -> Result<(), cairo::Error> {
    rounded_rect(cr, 0.0, 0.0, WIDTH, HEIGHT, 3.2);
    cr.set_source_rgba(r, g, b, 0.04);
    cr.fill_preserve()?;
    cr.set_source_rgba(r, g, b, 0.3);
    cr.set_line_width(0.5);
    cr.set_dash(&[2.0, 1.5], 0.0);
    cr.stroke()?;
    cr.set_dash(&[], 0.0);

    cr.set_source_rgba(r, g, b, 0.55);
    cr.select_font_face("Sans", FontSlant::Normal, FontWeight::Normal);
    cr.set_font_size(4.6);
    centered_text(cr, "Pick a tape from the shelf", WIDTH / 2.0, HEIGHT / 2.0 + 1.5)?;
    Ok(())
}

fn rounded_rect(cr: &Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -TAU / 4.0, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, TAU / 4.0);
    cr.arc(x + r, y + h - r, r, TAU / 4.0, TAU / 2.0);
    cr.arc(x + r, y + r, r, TAU / 2.0, TAU * 0.75);
    cr.close_path();
}

fn centered_text(cr: &Context, text: &str, cx: f64, baseline: f64) -> Result<(), cairo::Error> {
    let extents = cr.text_extents(text)?;
    cr.move_to(cx - extents.width() / 2.0 - extents.x_bearing(), baseline);
    cr.show_text(text)
}

/// `text`, shortened with an ellipsis until it is at most `max_width` wide.
fn fit_text(cr: &Context, text: &str, max_width: f64) -> Result<String, cairo::Error> {
    if cr.text_extents(text)?.x_advance() <= max_width {
        return Ok(text.to_owned());
    }
    let mut chars: Vec<char> = text.chars().collect();
    while !chars.is_empty() {
        chars.pop();
        let candidate = format!("{}…", chars.iter().collect::<String>().trim_end());
        if cr.text_extents(&candidate)?.x_advance() <= max_width {
            return Ok(candidate);
        }
    }
    Ok("…".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ontape_core::tape::{Cassette, TapeId};
    use std::time::Duration;

    fn status(position: Duration) -> TapeStatus {
        let cassette = Cassette::C60;
        let reels = Reels::for_cassette(cassette);
        TapeStatus {
            id: TapeId(1),
            cassette,
            facing: Side::A,
            position,
            side_len: cassette.side_len(),
            speed: 1.0,
            source: None,
            reel_radii: reels.reel_radii(position),
        }
    }

    #[test]
    fn reels_start_unturned_on_the_right_and_both_turn_forward() {
        let start = reel_angles(&status(Duration::ZERO));
        assert_eq!(start.1, 0.0);
        let later = reel_angles(&status(Duration::from_secs(60)));
        assert!(later.0 > start.0 && later.1 > start.1, "{start:?} → {later:?}");
    }

    #[test]
    fn an_empty_reel_turns_faster_than_a_full_one() {
        // Over the first minute of play the right reel is nearly empty and
        // turns more than the full left reel.
        let start = reel_angles(&status(Duration::ZERO));
        let later = reel_angles(&status(Duration::from_secs(60)));
        assert!(later.1 - start.1 > later.0 - start.0);
    }

    #[test]
    fn packs_fit_between_the_hubs() {
        // The two packs never overlap: their radii stay below the hub spacing.
        for secs in [0, 600, 900, 1800] {
            let radii = CassetteView::new(String::new(), &status(Duration::from_secs(secs))).reels.radii;
            assert!(radii.0 + radii.1 < RIGHT_HUB.0 - LEFT_HUB.0, "{secs}: {radii:?}");
            assert!(radii.0 >= HUB - 0.01 && radii.1 >= HUB - 0.01);
        }
    }

    /// How many frames a second get redrawn on a monitor ticking at `hz`.
    fn redrawn_per_second(hz: i64) -> usize {
        let mut last = None;
        (0..hz).map(|frame| frame * 1_000_000 / hz).filter(|&now| frame_due(last, now).then(|| last = Some(now)).is_some()).count()
    }

    #[test]
    fn a_moving_tape_is_redrawn_about_thirty_times_a_second() {
        assert!(frame_due(None, 0), "the first frame is always drawn");
        assert_eq!(redrawn_per_second(30), 30);
        assert_eq!(redrawn_per_second(60), 30);
        assert_eq!(redrawn_per_second(120), 30);
        for hz in [50, 75, 100, 144, 165, 240] {
            let fps = redrawn_per_second(hz);
            assert!((24..=30).contains(&fps), "{hz} Hz → {fps} fps");
        }
    }

    const INK: (f64, f64, f64) = (0.0, 0.0, 0.0);

    /// The red, green and blue of the finished picture at points given in
    /// the cassette's millimetres, in a `width` × `height` widget.
    fn colours(width: i32, height: i32, device_scale: i32, view: &CassetteView, points: &[(f64, f64)]) -> Vec<[u8; 3]> {
        let textures = Textures::draw(width, height, device_scale, Some(&view.label), INK).unwrap();
        let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, width, height).unwrap();
        scene(width, height, &textures, Some(&view.reels)).draw(&Context::new(&surface).unwrap());
        let at = placement(f64::from(width), f64::from(height)).unwrap();
        let stride = surface.stride() as usize;
        let data = surface.data().unwrap();
        let colour = |&(x, y): &(f64, f64)| {
            let pixel = (at.y + y * at.scale) as usize * stride + (at.x + x * at.scale) as usize * 4;
            [data[pixel + 2], data[pixel + 1], data[pixel]]
        };
        points.iter().map(colour).collect()
    }

    fn is_label(&[r, g, b]: &[u8; 3]) -> bool {
        r > 230 && g > 220 && b > 195
    }

    fn is_hub(&[r, g, b]: &[u8; 3]) -> bool {
        r > 200 && g > 200 && b > 195 && r < 235
    }

    fn is_pack(&[r, g, b]: &[u8; 3]) -> bool {
        (60..110).contains(&r) && r > g && g > b
    }

    fn is_dark_glass(&[r, g, b]: &[u8; 3]) -> bool {
        r < 60 && g < 60 && b < 60
    }

    #[test]
    fn drawing_does_not_fail() {
        let view = CassetteView::new("A rather long album title that will not fit".into(), &status(Duration::from_secs(300)));
        for (width, height, device_scale) in [(400, 260, 1), (400, 260, 2), (1512, 835, 1), (333, 777, 1)] {
            let textures = Textures::draw(width, height, device_scale, Some(&view.label), INK).unwrap();
            assert!(textures.tape.is_some());
            scene(width, height, &textures, Some(&view.reels));
        }
        // An empty deck, and a widget with no room for a cassette: only the shell's texture.
        let empty = Textures::draw(400, 260, 1, None, INK).unwrap();
        assert!(empty.tape.is_none());
        scene(400, 260, &empty, None);
        let tiny = Textures::draw(5, 5, 1, Some(&view.label), INK).unwrap();
        assert!(tiny.tape.is_none());
        scene(5, 5, &tiny, Some(&view.reels));
    }

    #[test]
    fn the_reels_show_through_the_window() {
        // Half way through the side the packs are equal and do not meet:
        // hub rims, bare glass between the packs, label above the window.
        let view = CassetteView::new(String::new(), &status(Duration::from_secs(900)));
        let between = ((LEFT_HUB.0 + RIGHT_HUB.0) / 2.0, LEFT_HUB.1);
        for device_scale in [1, 2] {
            let seen = colours(520, 330, device_scale, &view, &[(LEFT_HUB.0 + 9.0, LEFT_HUB.1), (RIGHT_HUB.0 - 9.0, RIGHT_HUB.1), between, (50.2, 17.5)]);
            assert!(is_hub(&seen[0]) && is_hub(&seen[1]), "hubs {seen:?}");
            assert!(is_dark_glass(&seen[2]), "glass {seen:?}");
            assert!(is_label(&seen[3]), "label {seen:?}");
        }
    }

    #[test]
    fn a_full_pack_shows_only_through_the_window() {
        // At the start the left pack is far wider than its hub opening. It
        // shows in the window to the right of the hub, but the label covers
        // it to the left of the opening and above the window.
        let view = CassetteView::new(String::new(), &status(Duration::ZERO));
        assert!(view.reels.radii.0 > HUB_OPENING + 3.0, "{:?}", view.reels.radii);
        let reach = HUB_OPENING + 1.5;
        let seen = colours(520, 330, 1, &view, &[(LEFT_HUB.0 + reach, LEFT_HUB.1), (LEFT_HUB.0 - reach, LEFT_HUB.1), (LEFT_HUB.0 + reach, WINDOW.1 - 1.0)]);
        assert!(is_pack(&seen[0]), "pack {seen:?}");
        assert!(is_label(&seen[1]) && is_label(&seen[2]), "label {seen:?}");
    }

    #[test]
    fn the_hubs_turn_with_the_tape() {
        // The notch on the right hub's rim starts at the top and has moved
        // on a quarter of a turn of tape later.
        let notch = (RIGHT_HUB.0, RIGHT_HUB.1 - NOTCH_ORBIT);
        let is_notch = |&[r, g, b]: &[u8; 3]| (100..135).contains(&r) && (100..135).contains(&g) && (100..140).contains(&b);
        let mut view = CassetteView::new(String::new(), &status(Duration::ZERO));
        assert_eq!(view.reels.angles.1, 0.0);
        assert!(is_notch(&colours(520, 330, 1, &view, &[notch])[0]));
        view.reels.angles.1 = TAU / 4.0;
        let seen = colours(520, 330, 1, &view, &[notch, (RIGHT_HUB.0 + NOTCH_ORBIT, RIGHT_HUB.1)]);
        assert!(is_hub(&seen[0]) && is_notch(&seen[1]), "{seen:?}");
    }

    #[test]
    fn the_teeth_turn_and_blur() {
        // A tooth reaches into the hub's dark hole straight below the centre.
        // A twelfth of a turn on, that spot lies between two teeth; winding
        // fast, the teeth are a faint ring there.
        let tooth = (RIGHT_HUB.0, RIGHT_HUB.1 + 5.4);
        let mut view = CassetteView::new(String::new(), &status(Duration::ZERO));
        assert!(is_hub(&colours(520, 330, 1, &view, &[tooth])[0]));
        view.reels.angles.1 = TAU / 12.0;
        assert!(is_dark_glass(&colours(520, 330, 1, &view, &[tooth])[0]));
        view.reels.speed = BLUR_SPEED * 2.0;
        let [r, g, b] = colours(520, 330, 1, &view, &[tooth])[0];
        assert!((70..130).contains(&r) && (70..130).contains(&g) && (70..130).contains(&b), "{:?}", [r, g, b]);
    }
}
