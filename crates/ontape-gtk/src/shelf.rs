//! One tape on the shelf: a row in the sidebar list.

use ontape_core::store::TapeSummary;
use ontape_core::tape::TapeId;
use relm4::adw::prelude::*;
use relm4::factory::{DynamicIndex, FactoryComponent, FactorySender};
use relm4::{adw, gtk};

use crate::transport::{fmt_time, side_name};

#[derive(Debug)]
pub struct ShelfRow {
    tape: TapeSummary,
    in_deck: bool,
}

#[derive(Debug)]
pub enum ShelfRowMsg {
    Update(TapeSummary, bool),
}

#[derive(Debug)]
pub enum ShelfRowOutput {
    Insert(TapeId),
    AskDelete(TapeId, String),
}

pub struct ShelfRowWidgets {
    row: adw::ActionRow,
    in_deck: gtk::Image,
}

impl ShelfRow {
    pub fn id(&self) -> TapeId {
        self.tape.id
    }

    fn subtitle(&self) -> String {
        format!("{} · side {} · {}", self.tape.cassette, side_name(self.tape.facing), fmt_time(self.tape.position))
    }
}

impl FactoryComponent for ShelfRow {
    type ParentWidget = gtk::ListBox;
    type CommandOutput = ();
    type Input = ShelfRowMsg;
    type Output = ShelfRowOutput;
    type Init = (TapeSummary, bool);
    type Root = adw::ActionRow;
    type Widgets = ShelfRowWidgets;
    type Index = DynamicIndex;

    fn init_model((tape, in_deck): Self::Init, _index: &DynamicIndex, _sender: FactorySender<Self>) -> Self {
        ShelfRow { tape, in_deck }
    }

    fn init_root(&self) -> adw::ActionRow {
        let row = adw::ActionRow::builder().activatable(true).build();
        row.set_use_markup(false);
        row
    }

    fn init_widgets(
        &mut self,
        _index: &DynamicIndex,
        row: adw::ActionRow,
        _returned_widget: &gtk::ListBoxRow,
        sender: FactorySender<Self>,
    ) -> ShelfRowWidgets {
        row.add_prefix(&gtk::Image::from_icon_name("media-tape-symbolic"));

        let in_deck = gtk::Image::from_icon_name("media-playback-start-symbolic");
        in_deck.set_tooltip_text(Some("In the deck"));
        in_deck.add_css_class("accent");
        row.add_suffix(&in_deck);

        let delete = gtk::Button::from_icon_name("user-trash-symbolic");
        delete.set_tooltip_text(Some("Throw the tape away"));
        delete.set_valign(gtk::Align::Center);
        delete.add_css_class("flat");
        row.add_suffix(&delete);

        let id = self.tape.id;
        let insert_sender = sender.clone();
        row.connect_activated(move |_| {
            let _ = insert_sender.output(ShelfRowOutput::Insert(id));
        });
        let title = self.tape.title.clone();
        delete.connect_clicked(move |_| {
            let _ = sender.output(ShelfRowOutput::AskDelete(id, title.clone()));
        });

        let widgets = ShelfRowWidgets { row, in_deck };
        self.show(&widgets);
        widgets
    }

    fn update(&mut self, msg: ShelfRowMsg, _sender: FactorySender<Self>) {
        match msg {
            ShelfRowMsg::Update(tape, in_deck) => {
                self.tape = tape;
                self.in_deck = in_deck;
            }
        }
    }

    fn update_view(&self, widgets: &mut ShelfRowWidgets, _sender: FactorySender<Self>) {
        self.show(widgets);
    }
}

impl ShelfRow {
    fn show(&self, widgets: &ShelfRowWidgets) {
        widgets.row.set_title(&self.tape.title);
        widgets.row.set_subtitle(&self.subtitle());
        widgets.in_deck.set_visible(self.in_deck);
    }
}

