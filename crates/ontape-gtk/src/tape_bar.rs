//! The bar under the cassette: the whole tape, side A then side B, with the
//! songs shaded, a tick where each one starts and a line at the head. It
//! only shows where the tape is; to get somewhere you still wind.

use std::cell::{Cell, RefCell};
use std::time::Duration;

use ontape_core::tape::Side;
use relm4::gtk::prelude::*;
use relm4::gtk::subclass::prelude::*;
use relm4::gtk::{self, gdk, glib, graphene, gsk};

use crate::tape_map::TapeMap;
use crate::transport::{fmt_time, side_name};

/// Room left at each end so the head line isn't cut off there.
const INSET: f32 = 2.0;
const TRACK_HEIGHT: f32 = 6.0;
const TICK_HEIGHT: f32 = 12.0;

glib::wrapper! {
    pub struct TapeBar(ObjectSubclass<imp::TapeBar>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl TapeBar {
    pub fn new() -> TapeBar {
        let bar: TapeBar = glib::Object::new();
        bar.add_css_class("tape-bar");
        bar.set_hexpand(true);
        bar.set_size_request(-1, 18);
        bar.set_has_tooltip(true);
        bar.connect_query_tooltip(|bar, x, _y, _keyboard, tooltip| {
            let text = bar.tooltip_at(x as f32);
            tooltip.set_text(text.as_deref());
            text.is_some()
        });
        bar
    }

    /// Shows this tape, or nothing for `None`.
    pub fn set_map(&self, map: Option<TapeMap>) {
        let shown = &self.imp().map;
        if *shown.borrow() != map {
            shown.replace(map);
            self.queue_draw();
        }
    }

    /// Moves the head to `position` on `side`.
    pub fn set_head(&self, side: Side, position: Duration) {
        let imp = self.imp();
        let Some(at) = imp
            .map
            .borrow()
            .as_ref()
            .map(|m| m.fraction(side, position))
        else {
            return;
        };
        if imp.facing.get() != Some(side) || imp.at.get() != at {
            imp.facing.set(Some(side));
            imp.at.set(at);
            self.queue_draw();
        }
    }

    fn span_width(&self) -> f32 {
        (self.width() as f32 - 2.0 * INSET).max(0.0)
    }

    fn tooltip_at(&self, x: f32) -> Option<String> {
        let width = self.span_width();
        if width <= 0.0 {
            return None;
        }
        let map = self.imp().map.borrow();
        let (side, span) = map.as_ref()?.span_at(f64::from((x - INSET) / width))?;
        let side = side_name(side);
        Some(match &span.song {
            Some(song) => match &song.title {
                Some(title) => format!(
                    "Side {side} · Song {} · {title} · starts {}",
                    song.number,
                    fmt_time(span.start)
                ),
                None => format!(
                    "Side {side} · Song {} · starts {}",
                    song.number,
                    fmt_time(span.start)
                ),
            },
            None => format!("Side {side} · silence"),
        })
    }
}

impl Default for TapeBar {
    fn default() -> TapeBar {
        TapeBar::new()
    }
}

fn with_alpha(colour: &gdk::RGBA, alpha: f32) -> gdk::RGBA {
    gdk::RGBA::new(
        colour.red(),
        colour.green(),
        colour.blue(),
        colour.alpha() * alpha,
    )
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct TapeBar {
        pub(super) map: RefCell<Option<TapeMap>>,
        pub(super) facing: Cell<Option<Side>>,
        pub(super) at: Cell<f64>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TapeBar {
        const NAME: &'static str = "OnTapeTapeBar";
        type Type = super::TapeBar;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for TapeBar {}

    impl WidgetImpl for TapeBar {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let bar = self.obj();
            let width = bar.span_width();
            let height = bar.height() as f32;
            let map = self.map.borrow();
            let Some(map) = map.as_ref() else { return };
            if width <= 0.0 || height <= 0.0 {
                return;
            }
            let ink = bar.color();
            let facing = self.facing.get();
            let x_of = |f: f64| INSET + width * f as f32;
            let mid = height / 2.0;
            let track_top = mid - TRACK_HEIGHT / 2.0;

            let track = graphene::Rect::new(INSET, track_top, width, TRACK_HEIGHT);
            let corner = graphene::Size::new(TRACK_HEIGHT / 2.0, TRACK_HEIGHT / 2.0);
            snapshot.push_rounded_clip(&gsk::RoundedRect::new(
                track, corner, corner, corner, corner,
            ));
            for side in [Side::A, Side::B] {
                // The side away from the head is the one you can't hear.
                let dim = if facing == Some(side) { 1.0 } else { 0.45 };
                for span in map.spans(side) {
                    let (from, to) = (
                        x_of(map.fraction(side, span.start)),
                        x_of(map.fraction(side, span.start + span.len)),
                    );
                    let alpha = if span.song.is_some() { 0.75 } else { 0.18 };
                    snapshot.append_color(
                        &with_alpha(&ink, alpha * dim),
                        &graphene::Rect::new(from, track_top, to - from, TRACK_HEIGHT),
                    );
                }
            }
            snapshot.pop();

            let tick = |x: f32, colour: &gdk::RGBA, w: f32, h: f32| {
                snapshot.append_color(
                    colour,
                    &graphene::Rect::new(x - w / 2.0, mid - h / 2.0, w, h),
                );
            };
            for side in [Side::A, Side::B] {
                let dim = if facing == Some(side) { 1.0 } else { 0.45 };
                for span in map.spans(side).iter().filter(|s| s.song.is_some()) {
                    tick(
                        x_of(map.fraction(side, span.start)),
                        &with_alpha(&ink, dim),
                        1.0,
                        TICK_HEIGHT,
                    );
                }
            }
            // Where A ends and B begins.
            tick(x_of(0.5), &with_alpha(&ink, 0.5), 2.0, height);
            if facing.is_some() {
                tick(x_of(self.at.get()), &ink, 3.0, height);
            }
        }
    }
}
