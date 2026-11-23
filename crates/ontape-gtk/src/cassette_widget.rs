//! The widget that shows the cassette.
//!
//! It keeps what cairo draws as textures, drawn again only when they would
//! look different, and puts every frame together from them. See
//! [`crate::cassette`] for the drawing itself.

use std::cell::RefCell;

use relm4::gtk::prelude::*;
use relm4::gtk::subclass::prelude::*;
use relm4::gtk::{self, glib};

use crate::cassette::{self, CassetteView, Label, Textures};

glib::wrapper! {
    pub struct CassetteWidget(ObjectSubclass<imp::CassetteWidget>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl CassetteWidget {
    pub fn new() -> CassetteWidget {
        glib::Object::new()
    }

    /// Shows the tape in the deck, or an empty deck for `None`. The widget
    /// is redrawn only if that changes the picture.
    pub fn set_view(&self, view: Option<CassetteView>) {
        let shown = &self.imp().view;
        if *shown.borrow() != view {
            shown.replace(view);
            self.queue_draw();
        }
    }
}

impl Default for CassetteWidget {
    fn default() -> CassetteWidget {
        CassetteWidget::new()
    }
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct CassetteWidget {
        pub(super) view: RefCell<Option<CassetteView>>,
        drawn: RefCell<Option<Drawn>>,
    }

    /// The textures, with everything their look depends on.
    struct Drawn {
        textures: Textures,
        width: i32,
        height: i32,
        scale: i32,
        label: Option<Label>,
        ink: (f64, f64, f64),
    }

    #[glib::object_subclass]
    impl ObjectSubclass for CassetteWidget {
        const NAME: &'static str = "OnTapeCassette";
        type Type = super::CassetteWidget;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for CassetteWidget {}

    impl WidgetImpl for CassetteWidget {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let widget = self.obj();
            let (width, height, scale) = (
                widget.width(),
                widget.height(),
                widget.scale_factor().max(1),
            );
            if width <= 0 || height <= 0 {
                return;
            }
            let view = self.view.borrow();
            let label = view.as_ref().map(|view| &view.label);
            let ink = widget.color();
            let ink = (
                f64::from(ink.red()),
                f64::from(ink.green()),
                f64::from(ink.blue()),
            );

            let mut drawn = self.drawn.borrow_mut();
            let current = |d: &Drawn| {
                (d.width, d.height, d.scale, d.label.as_ref(), d.ink)
                    == (width, height, scale, label, ink)
            };
            if !drawn.as_ref().is_some_and(current) {
                *drawn = Textures::draw(width, height, scale, label, ink).map(|textures| Drawn {
                    textures,
                    width,
                    height,
                    scale,
                    label: label.cloned(),
                    ink,
                });
            }
            // Without textures (cairo failed) the picture is missing until the next redraw.
            if let Some(drawn) = &*drawn {
                let reels = view.as_ref().map(|view| &view.reels);
                snapshot.append_node(cassette::scene(width, height, &drawn.textures, reels));
            }
        }
    }
}
