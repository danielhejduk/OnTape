use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use ontape_core::deck::{DeckState, DeckStatus};
use ontape_core::library::{self, RecordedTape, ScannedAlbum};
use ontape_core::player::Player;
use ontape_core::session::{self, DeckSession};
use ontape_core::tape::{Cassette, Direction, Source, Tape, TapeId, TrackId};
use relm4::adw::prelude::*;
use relm4::factory::FactoryVecDeque;
use relm4::gtk::{gdk, gio, glib};
use relm4::{Component, ComponentParts, ComponentSender, RelmWidgetExt, adw, gtk};

use crate::cassette::{self, CassetteView};
use crate::cassette_widget::CassetteWidget;
use crate::mpris::{self, MprisSlot, NowPlaying};
use crate::shelf::{ShelfRow, ShelfRowMsg, ShelfRowOutput};
use crate::transport::{self, Toggle, WindAction, side_name};

const TICK: Duration = Duration::from_millis(50);

#[derive(Debug)]
pub enum AppMsg {
    Tick,
    Play,
    Stop,
    PlayPause,
    Wind(Direction),
    WindPressed(Direction),
    WindReleased,
    Flip,
    Eject,
    ResetCounter,
    Insert(TapeId),
    AskDelete(TapeId, String),
    Delete(TapeId, String),
    ImportFolder,
    ImportFile,
    ImportChosen(PathBuf),
    NewBlank(Cassette),
}

#[derive(Debug)]
pub enum AppCmd {
    Scanned(Result<ScannedAlbum, String>),
}

pub struct App {
    session: Rc<RefCell<DeckSession>>,
    player: Option<Player>,
    shelf: FactoryVecDeque<ShelfRow>,
    mpris: MprisSlot,
    status: DeckStatus,
    was_moving: bool,
    /// Turns the reels on the cassette; installed only while the tape moves.
    reel_tick: Option<gtk::TickCallbackId>,
    label: Rc<RefCell<String>>,
    tape_title: Option<(TapeId, String)>,
    track_title: Option<(TrackId, Option<String>)>,
    track_line: String,
    cueing: bool,
    playback_error: Option<String>,
    toasts: adw::ToastOverlay,
    cassette: CassetteWidget,
    split: adw::NavigationSplitView,
}

pub struct AppWidgets {
    title: adw::WindowTitle,
    digits: [gtk::Label; 3],
    track: gtk::Label,
    rew: gtk::Button,
    play: gtk::Button,
    stop: gtk::Button,
    ff: gtk::Button,
    flip: gtk::Button,
    eject: gtk::Button,
}

impl Component for App {
    type Init = DeckSession;
    type Input = AppMsg;
    type Output = ();
    type CommandOutput = AppCmd;
    type Root = adw::ApplicationWindow;
    type Widgets = AppWidgets;

    fn init_root() -> adw::ApplicationWindow {
        adw::ApplicationWindow::builder()
            .title("OnTape")
            .default_width(1000)
            .default_height(660)
            .width_request(360)
            .height_request(480)
            .build()
    }

    fn init(
        session: DeckSession,
        root: adw::ApplicationWindow,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let session = Rc::new(RefCell::new(session));
        let label = Rc::new(RefCell::new(String::new()));
        let (player, playback_error) = match Player::open(1.0) {
            Ok(player) => (Some(player), None),
            Err(e) => (None, Some(format!("{e}. Playing silently."))),
        };

        // Shelf
        let shelf: FactoryVecDeque<ShelfRow> = FactoryVecDeque::builder()
            .launch(gtk::ListBox::default())
            .forward(sender.input_sender(), |output| match output {
                ShelfRowOutput::Insert(id) => AppMsg::Insert(id),
                ShelfRowOutput::AskDelete(id, title) => AppMsg::AskDelete(id, title),
            });
        let list = shelf.widget();
        list.add_css_class("navigation-sidebar");
        let placeholder = adw::StatusPage::builder()
            .icon_name("media-tape-symbolic")
            .title("No Tapes")
            .description("Use + to record an album or add a blank tape")
            .build();
        placeholder.add_css_class("compact");
        list.set_placeholder(Some(&placeholder));
        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(list)
            .build();
        let side_header = adw::HeaderBar::new();
        side_header.pack_start(&add_menu(&sender));
        let side_view = adw::ToolbarView::new();
        side_view.add_top_bar(&side_header);
        side_view.set_content(Some(&scrolled));
        let sidebar = adw::NavigationPage::builder()
            .title("Shelf")
            .child(&side_view)
            .build();

        // Deck
        let title = adw::WindowTitle::new("OnTape", "");
        let deck_header = adw::HeaderBar::new();
        deck_header.set_title_widget(Some(&title));

        let cassette = CassetteWidget::new();
        cassette.set_size_request(520, 330);
        cassette.set_hexpand(true);
        cassette.set_vexpand(true);
        let track = gtk::Label::builder()
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        track.add_css_class("track-line");

        let digits: [gtk::Label; 3] = std::array::from_fn(|_| {
            let digit = gtk::Label::new(Some("0"));
            digit.add_css_class("counter-digit");
            digit
        });
        let counter = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        counter.add_css_class("counter");
        counter.set_tooltip_text(Some("Tape counter"));
        for digit in &digits {
            counter.append(digit);
        }
        let reset = icon_button("view-refresh-symbolic", "Reset the counter to 000 (R)");
        reset.add_css_class("flat");
        reset.set_valign(gtk::Align::Center);
        let counter_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        counter_row.set_halign(gtk::Align::Center);
        counter_row.append(&counter);
        counter_row.append(&reset);

        let rew = wind_button(
            "media-seek-backward-symbolic",
            "Rewind (←); hold while playing to review",
            Direction::Backward,
            &sender,
        );
        let play = icon_button("media-playback-start-symbolic", "Play (Space)");
        let stop = icon_button("media-playback-stop-symbolic", "Stop (Space)");
        let ff = wind_button(
            "media-seek-forward-symbolic",
            "Fast-forward (→); hold while playing to cue",
            Direction::Forward,
            &sender,
        );
        let flip = icon_button("object-flip-horizontal-symbolic", "Turn the tape over (F)");
        let eject = icon_button("media-eject-symbolic", "Eject (E)");
        let keys = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        keys.add_css_class("linked");
        for button in [&rew, &play, &stop, &ff] {
            keys.append(button);
        }
        let tape_keys = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        tape_keys.add_css_class("linked");
        tape_keys.append(&flip);
        tape_keys.append(&eject);
        let transport_row = gtk::Box::new(gtk::Orientation::Horizontal, 18);
        transport_row.add_css_class("transport");
        transport_row.set_halign(gtk::Align::Center);
        transport_row.append(&keys);
        transport_row.append(&tape_keys);

        let deck_box = gtk::Box::new(gtk::Orientation::Vertical, 14);
        deck_box.set_margin_all(24);
        deck_box.append(&cassette);
        deck_box.append(&track);
        deck_box.append(&counter_row);
        deck_box.append(&transport_row);

        let deck_view = adw::ToolbarView::new();
        deck_view.add_top_bar(&deck_header);
        deck_view.set_content(Some(&deck_box));
        let content = adw::NavigationPage::builder()
            .title("Deck")
            .child(&deck_view)
            .build();

        let split = adw::NavigationSplitView::new();
        split.set_sidebar(Some(&sidebar));
        split.set_content(Some(&content));
        split.set_min_sidebar_width(260.0);
        split.set_max_sidebar_width(360.0);

        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&split));
        root.set_content(Some(&toasts));

        let narrow = adw::Breakpoint::new(
            adw::BreakpointCondition::parse("max-width: 640sp")
                .expect("valid breakpoint condition"),
        );
        narrow.add_setter(&split, "collapsed", Some(&true.to_value()));
        root.add_breakpoint(narrow);

        // ----- Wiring -----
        connect(&play, &sender, || AppMsg::Play);
        connect(&stop, &sender, || AppMsg::Stop);
        connect(&flip, &sender, || AppMsg::Flip);
        connect(&eject, &sender, || AppMsg::Eject);
        connect(&reset, &sender, || AppMsg::ResetCounter);
        root.add_controller(shortcuts(&root, &sender));
        {
            let session = session.clone();
            root.connect_close_request(move |_| {
                if let Ok(mut session) = session.try_borrow_mut() {
                    // Like taking your finger off the deck: stop, and remember where.
                    let _ = session.stop();
                    let _ = session.save();
                }
                glib::Propagation::Proceed
            });
        }
        {
            let input = sender.input_sender().clone();
            glib::timeout_add_local(TICK, move || {
                if input.send(AppMsg::Tick).is_ok() {
                    glib::ControlFlow::Continue
                } else {
                    glib::ControlFlow::Break
                }
            });
        }
        let mpris = MprisSlot::default();
        mpris::start(sender.input_sender().clone(), mpris.clone());

        let status = session.borrow().deck().status();
        let mut model = App {
            status,
            session,
            player,
            shelf,
            mpris,
            was_moving: false,
            reel_tick: None,
            label,
            tape_title: None,
            track_title: None,
            track_line: String::new(),
            cueing: false,
            playback_error,
            toasts,
            cassette,
            split,
        };
        model.refresh();
        model.refresh_shelf();
        if let Some(error) = &model.playback_error {
            model.toast(error);
        }

        let mut widgets = AppWidgets {
            title,
            digits,
            track,
            rew,
            play,
            stop,
            ff,
            flip,
            eject,
        };
        model.update_view(&mut widgets, sender);
        ComponentParts { model, widgets }
    }

    fn update(
        &mut self,
        msg: AppMsg,
        sender: ComponentSender<Self>,
        root: &adw::ApplicationWindow,
    ) {
        match msg {
            AppMsg::Tick => self.tick(),
            AppMsg::Play => self.act(DeckSession::play),
            AppMsg::Stop => self.act(DeckSession::stop),
            AppMsg::PlayPause => match transport::play_pause(self.status.state) {
                Toggle::Play => self.act(DeckSession::play),
                Toggle::Stop => self.act(DeckSession::stop),
            },
            AppMsg::Wind(dir) => self.act(|s| s.wind(dir)),
            AppMsg::WindPressed(dir) => match transport::wind_press(self.status.state) {
                WindAction::Cue => {
                    self.cueing = true;
                    self.act(|s| s.cue(dir));
                }
                WindAction::Wind => self.act(|s| s.wind(dir)),
            },
            AppMsg::WindReleased => {
                if std::mem::take(&mut self.cueing) {
                    self.act(DeckSession::release_cue);
                }
            }
            AppMsg::Flip => self.act(DeckSession::flip),
            AppMsg::Eject => {
                self.act(|s| s.eject().map(drop));
                self.refresh_shelf();
            }
            AppMsg::ResetCounter => self.act(DeckSession::reset_counter),
            AppMsg::Insert(id) => self.insert(id),
            AppMsg::AskDelete(id, title) => ask_delete(root, &sender, id, title),
            AppMsg::Delete(id, title) => self.delete(id, &title),
            AppMsg::ImportFolder => choose_import(root, &sender, true),
            AppMsg::ImportFile => choose_import(root, &sender, false),
            AppMsg::ImportChosen(path) => {
                self.toast("Reading the files…");
                sender.spawn_oneshot_command(move || {
                    AppCmd::Scanned(library::scan(&path, None).map_err(|e| e.to_string()))
                });
            }
            AppMsg::NewBlank(cassette) => {
                let title = format!("Blank {cassette}");
                let added = self
                    .session
                    .borrow_mut()
                    .store_mut()
                    .add_tape(&title, &Tape::blank(cassette));
                match added {
                    Ok(_) => self.toast(&format!("Added a blank {cassette}")),
                    Err(e) => self.toast(&e.to_string()),
                }
                self.refresh_shelf();
            }
        }
        self.refresh();
    }

    fn update_cmd(
        &mut self,
        cmd: AppCmd,
        _sender: ComponentSender<Self>,
        _root: &adw::ApplicationWindow,
    ) {
        match cmd {
            AppCmd::Scanned(Ok(album)) => {
                let recorded = library::record(self.session.borrow_mut().store_mut(), &album);
                match recorded {
                    Ok(tapes) => self.toast(&recorded_message(&album.title, &tapes)),
                    Err(e) => self.toast(&format!("Recording failed: {e}")),
                }
                self.refresh_shelf();
            }
            AppCmd::Scanned(Err(e)) => self.toast(&format!("Import failed: {e}")),
        }
    }

    fn update_view(&self, widgets: &mut AppWidgets, _sender: ComponentSender<Self>) {
        let counter = format!("{:03}", self.status.counter);
        for (digit, value) in widgets.digits.iter().zip(counter.chars()) {
            digit.set_label(value.encode_utf8(&mut [0; 4]));
        }

        match (&self.status.tape, &self.tape_title) {
            (Some(tape), Some((_, title))) => {
                widgets.title.set_title(title);
                widgets.title.set_subtitle(&format!(
                    "Side {} · {}",
                    side_name(tape.facing),
                    tape.cassette
                ));
            }
            _ => {
                widgets.title.set_title("OnTape");
                widgets.title.set_subtitle("No tape in the deck");
            }
        }
        widgets.track.set_label(&self.track_line);

        let state = self.status.state;
        let loaded = state != DeckState::Empty;
        widgets
            .play
            .set_class_active("latched", state == DeckState::Playing);
        widgets.ff.set_class_active(
            "latched",
            matches!(
                state,
                DeckState::Winding(Direction::Forward) | DeckState::Cueing(Direction::Forward)
            ),
        );
        widgets.rew.set_class_active(
            "latched",
            matches!(
                state,
                DeckState::Winding(Direction::Backward) | DeckState::Cueing(Direction::Backward)
            ),
        );
        for button in [
            &widgets.rew,
            &widgets.play,
            &widgets.stop,
            &widgets.ff,
            &widgets.flip,
            &widgets.eject,
        ] {
            button.set_sensitive(loaded);
        }
    }
}

impl App {
    /// Presses a deck button; a refusal ("no tape in the deck"…) becomes a toast.
    fn act(&mut self, press: impl FnOnce(&mut DeckSession) -> session::Result<()>) {
        let result = press(&mut self.session.borrow_mut());
        if let Err(e) = result {
            self.toast(&e.to_string());
        }
    }

    fn tick(&mut self) {
        let ticked = self.session.borrow_mut().tick();
        match ticked {
            Ok(events) => {
                for event in &events {
                    self.toast(&transport::describe(*event));
                }
            }
            Err(e) => self.toast(&format!("Saving failed: {e}")),
        }
        if let Some(player) = &mut self.player
            && let Err(e) = player.sync(&self.session.borrow())
        {
            let message = format!("Playback: {e}");
            // The player only reports each track once, but don't repeat ourselves either.
            if self.playback_error.as_ref() != Some(&message) {
                self.toast(&message);
                self.playback_error = Some(message);
            }
        }
    }

    /// Takes a tape off the shelf and puts it in, ejecting whatever was in.
    fn insert(&mut self, id: TapeId) {
        let inserted = {
            let mut session = self.session.borrow_mut();
            match session.deck().tape().map(|(current, _)| current) {
                Some(current) if current == id => Ok(()),
                Some(_) => session.eject().and_then(|_| session.insert(id)),
                None => session.insert(id),
            }
        };
        match inserted {
            Ok(()) => self.split.set_show_content(true),
            Err(e) => self.toast(&e.to_string()),
        }
        self.refresh_shelf();
    }

    fn delete(&mut self, id: TapeId, title: &str) {
        let in_deck = self
            .session
            .borrow()
            .deck()
            .tape()
            .is_some_and(|(current, _)| current == id);
        if in_deck {
            self.toast("Eject the tape before throwing it away");
            return;
        }
        let deleted = self.session.borrow().store().delete_tape(id);
        match deleted {
            Ok(()) => self.toast(&format!("Threw away “{title}”")),
            Err(e) => self.toast(&e.to_string()),
        }
        self.refresh_shelf();
    }

    /// Re-reads the deck for the display, the cassette and the media controls.
    fn refresh(&mut self) {
        let session = self.session.borrow();
        self.status = session.deck().status();

        let tape_id = self.status.tape.as_ref().map(|t| t.id);
        if self.tape_title.as_ref().map(|(id, _)| *id) != tape_id {
            self.tape_title =
                tape_id.map(|id| (id, session.store().tape_title(id).unwrap_or_default()));
            *self.label.borrow_mut() = self
                .tape_title
                .as_ref()
                .map(|(_, t)| t.clone())
                .unwrap_or_default();
        }

        self.track_line = match self.status.tape.as_ref().map(|t| (t, t.source)) {
            None => String::new(),
            Some((_, None)) => "End of the side".to_owned(),
            Some((_, Some(Source::Silence))) => "Blank tape".to_owned(),
            Some((tape, Some(Source::Track { track, offset }))) => {
                if self.track_title.as_ref().map(|(id, _)| *id) != Some(track) {
                    self.track_title =
                        Some((track, session.store().track_title(track).ok().flatten()));
                }
                let song = session.deck().tape().map_or(0, |(_, t)| {
                    transport::song_number(t, tape.facing, tape.position)
                });
                match self
                    .track_title
                    .as_ref()
                    .and_then(|(_, name)| name.as_deref())
                {
                    Some(name) => format!("Song {song} · {name} · {}", transport::fmt_time(offset)),
                    None => format!("Song {song} · {}", transport::fmt_time(offset)),
                }
            }
        };
        drop(session);

        if let Some(mpris) = &*self.mpris.borrow() {
            let title = match self.status.tape.as_ref().and_then(|t| t.source) {
                Some(Source::Track { .. }) => {
                    self.track_title.as_ref().and_then(|(_, name)| name.clone())
                }
                _ => None,
            };
            mpris.show(NowPlaying {
                status: mpris::playback_status(self.status.state),
                title,
                album: self.tape_title.as_ref().map(|(_, t)| t.clone()),
            });
        }

        let moving = matches!(
            self.status.state,
            DeckState::Playing | DeckState::Winding(_) | DeckState::Cueing(_)
        );
        if self.was_moving && !moving {
            // The tape stopped somewhere new: show that on the shelf.
            self.refresh_shelf();
        }
        self.was_moving = moving;
        self.show_cassette(moving);
    }

    /// Keeps the cassette picture up to date: about 30 times a second while
    /// the tape moves, straight from the deck, and otherwise whenever the
    /// deck is refreshed. The widget redraws only what looks different.
    fn show_cassette(&mut self, moving: bool) {
        if !moving {
            if let Some(tick) = self.reel_tick.take() {
                tick.remove();
            }
            let view = self
                .status
                .tape
                .as_ref()
                .map(|t| CassetteView::new(self.label.borrow().clone(), t));
            self.cassette.set_view(view);
        } else if self.reel_tick.is_none() {
            let (session, label) = (self.session.clone(), self.label.clone());
            let drawn = Cell::new(None);
            self.reel_tick = Some(self.cassette.add_tick_callback(move |cassette, clock| {
                let now = clock.frame_time();
                if cassette::frame_due(drawn.get(), now)
                    && let Ok(session) = session.try_borrow()
                {
                    drawn.set(Some(now));
                    cassette.set_view(cassette_view(&session, &label.borrow()));
                }
                glib::ControlFlow::Continue
            }));
        }
    }

    fn refresh_shelf(&mut self) {
        let (tapes, inserted) = {
            let session = self.session.borrow();
            (
                session.store().list_tapes(),
                session.deck().tape().map(|(id, _)| id),
            )
        };
        let tapes = match tapes {
            Ok(tapes) => tapes,
            Err(e) => {
                self.toast(&e.to_string());
                return;
            }
        };
        let mut guard = self.shelf.guard();
        let same_tapes = guard.len() == tapes.len()
            && guard
                .iter()
                .zip(&tapes)
                .all(|(row, tape)| row.id() == tape.id);
        if same_tapes {
            for (i, tape) in tapes.into_iter().enumerate() {
                let in_deck = inserted == Some(tape.id);
                guard.send(i, ShelfRowMsg::Update(tape, in_deck));
            }
        } else {
            guard.clear();
            for tape in tapes {
                let in_deck = inserted == Some(tape.id);
                guard.push_back((tape, in_deck));
            }
        }
    }

    fn toast(&self, text: &str) {
        // Core errors start lowercase ("no tape in the deck"); toasts read as sentences.
        let mut chars = text.chars();
        let text: String = chars
            .next()
            .map(|c| c.to_uppercase().chain(chars).collect())
            .unwrap_or_default();
        let toast = adw::Toast::new(&text);
        toast.set_use_markup(false);
        toast.set_timeout(3);
        self.toasts.add_toast(toast);
    }
}

/// The tape in the deck as the cassette drawing sees it, right now.
fn cassette_view(session: &DeckSession, title: &str) -> Option<CassetteView> {
    session
        .deck()
        .status()
        .tape
        .as_ref()
        .map(|tape| CassetteView::new(title.to_owned(), tape))
}

fn recorded_message(title: &str, tapes: &[RecordedTape]) -> String {
    match tapes {
        [tape] => format!("Recorded “{title}” on a {}", tape.cassette),
        _ => format!("Recorded “{title}” on {} tapes", tapes.len()),
    }
}

fn icon_button(icon: &str, tooltip: &str) -> gtk::Button {
    let button = gtk::Button::from_icon_name(icon);
    button.set_tooltip_text(Some(tooltip));
    button
}

fn connect(button: &gtk::Button, sender: &ComponentSender<App>, msg: fn() -> AppMsg) {
    let sender = sender.clone();
    button.connect_clicked(move |_| sender.input(msg()));
}

/// ◀◀ or ▶▶. With the pointer it reacts to press and release, so it can be
/// held for cue/review; from the keyboard it just winds.
fn wind_button(
    icon: &str,
    tooltip: &str,
    dir: Direction,
    sender: &ComponentSender<App>,
) -> gtk::Button {
    let button = icon_button(icon, tooltip);
    let gesture = gtk::GestureClick::new();
    gesture.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let sender = sender.clone();
        gesture.connect_pressed(move |gesture, _, _, _| {
            // Claiming the press keeps the button from also sending "clicked".
            gesture.set_state(gtk::EventSequenceState::Claimed);
            sender.input(AppMsg::WindPressed(dir));
        });
    }
    {
        let sender = sender.clone();
        gesture.connect_released(move |_, _, _, _| sender.input(AppMsg::WindReleased));
    }
    {
        let sender = sender.clone();
        gesture.connect_cancel(move |_, _| sender.input(AppMsg::WindReleased));
    }
    button.add_controller(gesture);
    let sender = sender.clone();
    button.connect_clicked(move |_| sender.input(AppMsg::Wind(dir)));
    button
}

/// The + menu: record albums and add blank tapes.
fn add_menu(sender: &ComponentSender<App>) -> gtk::MenuButton {
    let popover = gtk::Popover::new();
    let items = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let item = |label: &str, msg: Box<dyn Fn() -> AppMsg>| {
        let button = gtk::Button::with_label(label);
        button.add_css_class("flat");
        if let Some(text) = button.child().and_downcast::<gtk::Label>() {
            text.set_xalign(0.0);
        }
        let (sender, popover) = (sender.clone(), popover.clone());
        button.connect_clicked(move |_| {
            popover.popdown();
            sender.input(msg());
        });
        button
    };
    items.append(&item(
        "Record Album Folder…",
        Box::new(|| AppMsg::ImportFolder),
    ));
    items.append(&item("Record Audio File…", Box::new(|| AppMsg::ImportFile)));
    items.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    for cassette in Cassette::ALL {
        items.append(&item(
            &format!("New Blank {cassette}"),
            Box::new(move || AppMsg::NewBlank(cassette)),
        ));
    }
    popover.set_child(Some(&items));
    gtk::MenuButton::builder()
        .icon_name("list-add-symbolic")
        .tooltip_text("Add tapes")
        .popover(&popover)
        .build()
}

/// Space, ←, →, F, E and R work anywhere in the window (but not in dialogs).
fn shortcuts(
    root: &adw::ApplicationWindow,
    sender: &ComponentSender<App>,
) -> gtk::EventControllerKey {
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let (root, sender) = (root.clone(), sender.clone());
    keys.connect_key_pressed(move |_, key, _, modifiers| {
        let busy = root.visible_dialog().is_some()
            || modifiers.intersects(
                gdk::ModifierType::CONTROL_MASK
                    | gdk::ModifierType::ALT_MASK
                    | gdk::ModifierType::SUPER_MASK,
            );
        let msg = match key {
            _ if busy => return glib::Propagation::Proceed,
            gdk::Key::space => AppMsg::PlayPause,
            gdk::Key::Left => AppMsg::Wind(Direction::Backward),
            gdk::Key::Right => AppMsg::Wind(Direction::Forward),
            gdk::Key::f | gdk::Key::F => AppMsg::Flip,
            gdk::Key::e | gdk::Key::E => AppMsg::Eject,
            gdk::Key::r | gdk::Key::R => AppMsg::ResetCounter,
            _ => return glib::Propagation::Proceed,
        };
        sender.input(msg);
        glib::Propagation::Stop
    });
    keys
}

fn choose_import(root: &adw::ApplicationWindow, sender: &ComponentSender<App>, folder: bool) {
    let dialog = gtk::FileDialog::builder().modal(true).build();
    let sender = sender.clone();
    let chosen = move |result: Result<gio::File, glib::Error>| {
        if let Some(path) = result.ok().and_then(|file| file.path()) {
            sender.input(AppMsg::ImportChosen(path));
        }
    };
    if folder {
        dialog.set_title("Record Album Folder");
        dialog.select_folder(Some(root), gio::Cancellable::NONE, chosen);
    } else {
        dialog.set_title("Record Audio File");
        let audio = gtk::FileFilter::new();
        audio.set_name(Some("Audio"));
        audio.add_mime_type("audio/*");
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&audio);
        dialog.set_filters(Some(&filters));
        dialog.open(Some(root), gio::Cancellable::NONE, chosen);
    }
}

fn ask_delete(
    root: &adw::ApplicationWindow,
    sender: &ComponentSender<App>,
    id: TapeId,
    title: String,
) {
    let dialog = adw::AlertDialog::new(
        Some("Throw the Tape Away?"),
        Some(&format!(
            "“{title}” and where you got to on it will be gone. The audio files stay."
        )),
    );
    dialog.add_response("cancel", "_Keep");
    dialog.add_response("delete", "_Throw Away");
    dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    let sender = sender.clone();
    dialog.connect_response(None, move |_, response| {
        if response == "delete" {
            sender.input(AppMsg::Delete(id, title.clone()));
        }
    });
    dialog.present(Some(root));
}
