//! The Settings window: a sidebar of categories with a search field, and a
//! page of cards for the category picked.
//!
//! Every row is declared once, in `pages`, with its title, its subtitle and
//! a few hidden keywords beside the code that draws its control. The page draws
//! from that list and so does the search, which is what keeps the two from
//! drifting apart: there is no second list of "things you can search for" to
//! forget to update, and a row added to a page is findable the moment it is.
//!
//! The categories are arranged by what someone opening the window is trying
//! to do - change how it looks, how a window behaves, how typing works, what a
//! session connects to - rather than by which part of the code reads the
//! setting. What is seldom needed on a page sits on a sub-page of its own,
//! reached from a row with a chevron, and named for what is on it.
//!
//! No OK or Cancel. Every change is applied the moment it is made, so it can
//! be judged against the terminal behind the window: a Cancel would be lying
//! about what it did.
//!
//! # The managers' pages
//!
//! Themes, Screen saver and Macros used to be windows of their own, opened
//! from here. They are pages now, in [`themes`], [`screensaver`] and
//! [`macros`], declared like every other: a list leads to a detail sub-page,
//! and a sub-page can lead to one of its own (`Page::parent`), with the way
//! back named for where it goes. What has to stay in view while the page
//! scrolls - a preview of the theme being edited, the screen saver's little
//! monitor - is the page's `top`, drawn between the header and the cards.
//!
//! A row that edits "the selected one" - a colour of the theme being edited,
//! a field of the open macro - is `contextual`: the search finds it like any
//! other, but in the results it is only a way to its page, since out there
//! nothing says which theme or macro its control would be changing.

use std::sync::OnceLock;

use egui::{Color32, Context, Key, KeyboardShortcut, Modifiers, PointerButton, Ui};

use crate::config::profile::{LogMode, Remote};
use crate::config::servers::{Server, ServerList, Target};
use crate::config::{CursorStyle, IntellisenseMode, Profile, Settings, Theme};
use crate::features::macros::MacroGroup;
use crate::i18n::{tr, tr1, Lang};
use crate::term::Encoding;
use crate::ui::icons::Symbol;
use crate::ui::panels::{PanelState, UiRequest};
use crate::ui::prefs::{self, Card, Row};

pub mod macros;
pub mod screensaver;
pub mod themes;

/// Shown in the font picker for "whatever egui ships with".
const BUILT_IN_FONT: &str = "Built-in monospace";

/// The widest the content column grows, however wide the window.
const COLUMN: f32 = 640.0;

/// How long a row a search result led to stays lit, in seconds.
const HIGHLIGHT_SECS: f64 = 1.6;

/// The categories down the sidebar, in the order they run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Category {
    #[default]
    General,
    Appearance,
    Themes,
    ScreenSaver,
    Windows,
    Terminal,
    Keyboard,
    Macros,
    Sessions,
    About,
}

impl Category {
    pub const ALL: [Category; 10] = [
        Category::General,
        Category::Appearance,
        Category::Themes,
        Category::ScreenSaver,
        Category::Windows,
        Category::Terminal,
        Category::Keyboard,
        Category::Macros,
        Category::Sessions,
        Category::About,
    ];

    /// The category's name, untranslated.
    pub fn label(self) -> &'static str {
        match self {
            Category::General => "General",
            Category::Appearance => "Appearance",
            Category::Themes => "Themes",
            Category::ScreenSaver => "Screen saver",
            Category::Windows => "Window and tabs",
            Category::Terminal => "Terminal",
            Category::Keyboard => "Keyboard and editing",
            Category::Macros => "Macros",
            Category::Sessions => "Sessions",
            Category::About => "About",
        }
    }

    fn symbol(self) -> Symbol {
        match self {
            Category::General => Symbol::Gear,
            Category::Appearance => Symbol::Contrast,
            Category::Themes => Symbol::Swatch,
            Category::ScreenSaver => Symbol::Display,
            Category::Windows => Symbol::Windows,
            Category::Terminal => Symbol::Prompt,
            Category::Keyboard => Symbol::Pencil,
            Category::Macros => Symbol::Bolt,
            Category::Sessions => Symbol::Person,
            Category::About => Symbol::Info,
        }
    }

    /// The tile behind the symbol. Fixed rather than themed, as System
    /// Settings' are: the colour is how a category is found again without
    /// reading the list, which only works if it is the same in every theme.
    fn tile(self) -> egui::Color32 {
        let rgb = egui::Color32::from_rgb;
        match self {
            Category::General => rgb(0x8e, 0x8e, 0x93),
            Category::Appearance => rgb(0x00, 0x7a, 0xff),
            Category::Themes => rgb(0xaf, 0x52, 0xde),
            Category::ScreenSaver => rgb(0x30, 0xb0, 0xc7),
            Category::Windows => rgb(0x00, 0x7a, 0xff).lerp_to_gamma(rgb(0x30, 0xb0, 0xc7), 0.5),
            Category::Terminal => rgb(0x3a, 0x3a, 0x3c),
            Category::Keyboard => rgb(0xff, 0x95, 0x00),
            Category::Macros => rgb(0xe8, 0xb0, 0x00),
            Category::Sessions => rgb(0x34, 0xc7, 0x59),
            Category::About => rgb(0x8e, 0x8e, 0x93).lerp_to_gamma(rgb(0x58, 0x56, 0xd6), 0.5),
        }
    }

    /// Opens a cluster of related entries, with a gap above it.
    fn starts_group(self) -> bool {
        matches!(
            self,
            Category::Windows | Category::Terminal | Category::Sessions | Category::About
        )
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|&c| c == self).unwrap_or(0)
    }

    fn from_index(index: usize) -> Self {
        Self::ALL.get(index).copied().unwrap_or_default()
    }
}

/// Where the window is: a category's page, or one of its sub-pages.
///
/// Remembered in [`PanelState`] rather than in settings.toml: coming back to
/// the page you were on is a convenience for this session, not a preference.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Route {
    pub category: Category,
    /// The sub-page's key, or `None` for the category's own page.
    pub sub: Option<&'static str>,
    /// Which one, for a sub-page that is one of many - a profile.
    pub index: usize,
}

impl Route {
    pub const fn page(category: Category) -> Self {
        Route {
            category,
            sub: None,
            index: 0,
        }
    }

    const fn sub(category: Category, sub: &'static str) -> Self {
        Route {
            category,
            sub: Some(sub),
            index: 0,
        }
    }
}

/// Where the macro manager's shortcut leads: the list of macros, which is
/// what the manager window it used to open showed first.
pub const MACROS_PAGE: Route = Route::page(Category::Macros);

/// What a row's drawing code is handed: everything Settings edits or reports.
pub(crate) struct Ctx<'a> {
    settings: &'a mut Settings,
    /// Edited in place, so the terminal behind the window repaints in the
    /// colour being dragged; what has to reach the disk goes out as a request.
    themes: &'a mut Vec<Theme>,
    /// The macros, likewise edited in place and written out by request.
    macro_groups: &'a mut Vec<MacroGroup>,
    state: &'a mut PanelState,
    instances: &'a [String],
    servers: &'a ServerList,
    /// Actions - opening a folder, checking for updates, saving a theme. Kept
    /// apart from `changed`: an action is not an edit, and must not make the
    /// app rewrite settings.toml.
    requests: Vec<UiRequest>,
    changed: bool,
    /// A colour of the theme being edited changed this frame.
    theme_edited: bool,
    /// Where to go once this frame is drawn. Not at once: the page being
    /// drawn is still borrowed from the route it is leaving.
    go: Option<Route>,
    /// The index of the route being drawn, for a sub-page that is one of many.
    index: usize,
}

/// How a row draws.
#[derive(Clone, Copy)]
enum Kind {
    /// A switch bound to one setting - most rows.
    Toggle(fn(&mut Settings) -> &mut bool),
    /// A control on the right of the row's words.
    Control(fn(&mut Ui, &mut Ctx<'_>)),
    /// A row that opens another page: a sub-page, or another category.
    Link(Route),
    /// Rows of its own, for what a single declaration cannot know the shape
    /// of in advance: one per profile, one per shell found.
    Rows(fn(&mut Card<'_>, &mut Ctx<'_>)),
    /// Words with no control - a tip.
    Note,
    /// A swatch for one colour of the theme being edited. `None` when the
    /// colour is not there to edit just now - a gradient's end with no
    /// gradient.
    Colour(fn(&mut Theme) -> Option<&mut Color32>),
    /// A colour the theme may leave to its button style: the swatch shows
    /// what will be painted (the second function, when it is unset), and a
    /// reset button gives it back to the style.
    Fallback(
        fn(&mut Theme) -> &mut Option<Color32>,
        fn(&Theme) -> Color32,
    ),
}

/// One row, as declared.
#[derive(Clone, Copy)]
struct Item {
    /// Unique across every page: the search leads to a row by it.
    key: &'static str,
    title: &'static str,
    subtitle: Option<&'static str>,
    hint: Option<&'static str>,
    /// Searched but never shown. Both languages, since they are not
    /// translated: what someone might type looking for this row that its words
    /// do not say.
    keywords: &'static [&'static str],
    kind: Kind,
    /// Whether the row applies at all just now - a proxy password with no
    /// proxy to give it to does not.
    when: Option<fn(&Ctx<'_>) -> bool>,
    /// The row edits the theme or macro its page is open on, so a search
    /// result shows it only as the way to that page. See the module's notes.
    contextual: bool,
}

impl Item {
    fn new(key: &'static str, title: &'static str, kind: Kind) -> Self {
        Item {
            key,
            title,
            subtitle: None,
            hint: None,
            keywords: &[],
            kind,
            when: None,
            contextual: matches!(kind, Kind::Colour(_) | Kind::Fallback(..)),
        }
    }

    fn colour(
        key: &'static str,
        title: &'static str,
        get: fn(&mut Theme) -> Option<&mut Color32>,
    ) -> Self {
        Self::new(key, title, Kind::Colour(get))
    }

    fn fallback(
        key: &'static str,
        title: &'static str,
        get: fn(&mut Theme) -> &mut Option<Color32>,
        or: fn(&Theme) -> Color32,
    ) -> Self {
        Self::new(key, title, Kind::Fallback(get, or))
    }

    fn contextual(mut self) -> Self {
        self.contextual = true;
        self
    }

    fn toggle(key: &'static str, title: &'static str, get: fn(&mut Settings) -> &mut bool) -> Self {
        Self::new(key, title, Kind::Toggle(get))
    }

    fn control(key: &'static str, title: &'static str, draw: fn(&mut Ui, &mut Ctx<'_>)) -> Self {
        Self::new(key, title, Kind::Control(draw))
    }

    fn link(key: &'static str, title: &'static str, to: Route) -> Self {
        Self::new(key, title, Kind::Link(to))
    }

    fn rows(key: &'static str, title: &'static str, draw: fn(&mut Card<'_>, &mut Ctx<'_>)) -> Self {
        Self::new(key, title, Kind::Rows(draw))
    }

    fn sub(mut self, subtitle: &'static str) -> Self {
        self.subtitle = Some(subtitle);
        self
    }

    fn hint(mut self, hint: &'static str) -> Self {
        self.hint = Some(hint);
        self
    }

    fn keys(mut self, keywords: &'static [&'static str]) -> Self {
        self.keywords = keywords;
        self
    }

    fn when(mut self, when: fn(&Ctx<'_>) -> bool) -> Self {
        self.when = Some(when);
        self
    }

    fn applies(&self, ctx: &Ctx<'_>) -> bool {
        self.when.is_none_or(|when| when(ctx))
    }
}

/// A heading, the card under it, and the note after it.
struct Section {
    heading: Option<&'static str>,
    footer: Option<&'static str>,
    items: Vec<Item>,
    /// Drawn by this instead of as a card of `items` - for a run of cards
    /// whose number is not known in advance, one per macro group. Not
    /// searched, since what it draws is not declared.
    custom: Option<fn(&mut Ui, &mut Ctx<'_>)>,
    /// Whether the card's controls take input just now - an organization
    /// macro's fields are shown, not edited.
    enabled: Option<fn(&Ctx<'_>) -> bool>,
}

fn section(heading: &'static str, items: Vec<Item>) -> Section {
    Section {
        heading: Some(heading),
        ..untitled(items)
    }
}

/// A card with no heading over it - the one a page opens with, under its
/// title, which already says what it is about.
fn untitled(items: Vec<Item>) -> Section {
    Section {
        heading: None,
        footer: None,
        items,
        custom: None,
        enabled: None,
    }
}

/// A section drawn by `draw` rather than declared.
fn drawn(draw: fn(&mut Ui, &mut Ctx<'_>)) -> Section {
    Section {
        custom: Some(draw),
        ..untitled(Vec::new())
    }
}

impl Section {
    fn footer(mut self, footer: &'static str) -> Self {
        self.footer = Some(footer);
        self
    }

    fn enabled(mut self, enabled: fn(&Ctx<'_>) -> bool) -> Self {
        self.enabled = Some(enabled);
        self
    }
}

/// What a page shows under its header.
enum Body {
    /// Declared rows: drawn by the window, and searched.
    Sections(Vec<Section>),
    /// Drawn by the function, handed the whole content area under the header -
    /// for a page whose shape is not a list of rows. Not searched: what it
    /// draws is not declared. A page that should still be findable declares
    /// a row for itself on its parent.
    Custom(fn(&mut Ui, &mut Ctx<'_>)),
}

struct Page {
    category: Category,
    /// `None` for the category's own page; the sub-page's key otherwise.
    sub: Option<&'static str>,
    title: &'static str,
    /// A title worked out when the page is shown, for a sub-page that is one
    /// of many and named for which: a profile is called by its name.
    heading: Option<fn(&Ctx<'_>) -> String>,
    /// The sub-page the way back leads to, for a sub-page of a sub-page;
    /// `None` leads back to the category's own page.
    parent: Option<&'static str>,
    /// Drawn under the header and above the scroll, so it stays in view
    /// while the cards move under it: the preview of what they are changing.
    top: Option<fn(&mut Ui, &mut Ctx<'_>)>,
    body: Body,
}

fn page(category: Category, sections: Vec<Section>) -> Page {
    Page {
        category,
        sub: None,
        title: category.label(),
        heading: None,
        parent: None,
        top: None,
        body: Body::Sections(sections),
    }
}

fn subpage(
    category: Category,
    sub: &'static str,
    title: &'static str,
    sections: Vec<Section>,
) -> Page {
    Page {
        sub: Some(sub),
        title,
        ..page(category, sections)
    }
}

impl Page {
    fn route(&self) -> Route {
        Route {
            category: self.category,
            sub: self.sub,
            index: 0,
        }
    }

    fn with_top(mut self, top: fn(&mut Ui, &mut Ctx<'_>)) -> Self {
        self.top = Some(top);
        self
    }

    fn with_heading(mut self, heading: fn(&Ctx<'_>) -> String) -> Self {
        self.heading = Some(heading);
        self
    }

    fn under(mut self, parent: &'static str) -> Self {
        self.parent = Some(parent);
        self
    }

    /// Where the way back from this page leads; `None` on a category's own
    /// page, which has nowhere further back to go.
    fn back(&self) -> Option<Route> {
        self.sub?;
        Some(match self.parent {
            Some(parent) => Route::sub(self.category, parent),
            None => Route::page(self.category),
        })
    }

    fn sections(&self) -> &[Section] {
        match &self.body {
            Body::Sections(sections) => sections,
            Body::Custom(_) => &[],
        }
    }

    /// Where the page is, for a search result: "About > Proxy".
    fn breadcrumb(&self, tr: impl Fn(&'static str) -> &'static str) -> String {
        match self.sub {
            None => tr(self.title).to_owned(),
            Some(_) => format!("{} \u{203a} {}", tr(self.category.label()), tr(self.title)),
        }
    }
}

fn find_page(route: Route) -> Option<&'static Page> {
    pages()
        .iter()
        .find(|page| page.category == route.category && page.sub == route.sub)
}

/// Whether `item` answers `query`, with `tr` as the language to read it in.
///
/// The English is searched as well as the translation, so someone using the
/// Portuguese interface who knows the English name still finds it. The page
/// and the section are searched too: "proxy" should bring up the proxy's
/// password, whose own words do not say proxy.
fn item_matches(
    query: &str,
    page: &Page,
    section: &Section,
    item: &Item,
    tr: impl Fn(&'static str) -> &'static str,
) -> bool {
    let mut haystacks: Vec<&str> = Vec::with_capacity(12 + item.keywords.len());
    for text in [
        Some(item.title),
        item.subtitle,
        section.heading,
        Some(page.title),
        Some(page.category.label()),
    ]
    .into_iter()
    .flatten()
    {
        haystacks.push(text);
        haystacks.push(tr(text));
    }
    haystacks.extend_from_slice(item.keywords);
    prefs::matches(query, &haystacks)
}

/// Every row that answers `query`, page by page, in the order they are shown.
fn search(
    query: &str,
    tr: impl Fn(&'static str) -> &'static str + Copy,
) -> Vec<(&'static Page, Vec<&'static Item>)> {
    pages()
        .iter()
        .filter_map(|page| {
            let hits: Vec<&Item> = page
                .sections()
                .iter()
                .flat_map(|section| {
                    section
                        .items
                        .iter()
                        .filter(move |item| item_matches(query, page, section, item, tr))
                })
                .collect();
            (!hits.is_empty()).then_some((page, hits))
        })
        .collect()
}

/// What a row needs to know about the view it is drawn in.
struct View {
    /// The row a search result led to, and how lit it still is.
    highlight: Option<(&'static str, f32)>,
    /// The row to bring into view, once.
    scroll_to: Option<&'static str>,
    /// Rows are search results: their words lead to their page.
    results: bool,
    /// The row a click on a result asked to be shown, on the page `Ctx::go`
    /// is set to.
    lead_to: Option<&'static str>,
}

fn draw_item(card: &mut Card<'_>, page: &Page, item: &Item, ctx: &mut Ctx<'_>, view: &mut View) {
    let highlight = match view.highlight {
        Some((key, strength)) if key == item.key => strength,
        _ => 0.0,
    };
    let spec = Row::new(tr(item.title))
        .subtitle(item.subtitle.map(tr))
        .hint(item.hint.map(tr))
        .highlight(highlight)
        .link(view.results);
    if view.results && item.contextual {
        let shown = card.row(spec, |_| ());
        if shown.label.clicked() {
            ctx.go = Some(page.route());
            view.lead_to = Some(item.key);
        }
        return;
    }
    let shown = match item.kind {
        Kind::Colour(get) => {
            let shown = themes::colour_item(card, spec, ctx, get);
            shown.map(|s| (s.label, s.rect))
        }
        Kind::Fallback(get, or) => {
            let shown = themes::fallback_item(card, spec, ctx, get, or);
            shown.map(|s| (s.label, s.rect))
        }
        Kind::Toggle(get) => {
            let shown = card.toggle(spec, get(ctx.settings));
            ctx.changed |= shown.inner;
            Some((shown.label, shown.rect))
        }
        Kind::Control(draw) => {
            let shown = card.row(spec, |ui| draw(ui, ctx));
            Some((shown.label, shown.rect))
        }
        Kind::Link(to) => {
            // Leads somewhere in a result as on a page: to where it goes, not
            // to the page it sits on.
            if card.nav(spec).clicked() {
                ctx.go = Some(to);
            }
            None
        }
        Kind::Note => {
            let shown = card.row(spec, |_| ());
            Some((shown.label, shown.rect))
        }
        Kind::Rows(draw) => {
            draw(card, ctx);
            None
        }
    };
    if let Some((label, rect)) = shown {
        if view.results && label.clicked() {
            ctx.go = Some(page.route());
            view.lead_to = Some(item.key);
        }
        if view.scroll_to == Some(item.key) {
            card.ui().scroll_to_rect(rect, Some(egui::Align::Center));
            view.scroll_to = None;
        }
    }
}

/// The sections of a declared page, each a heading, a card and a footer.
fn draw_sections(ui: &mut Ui, page: &Page, c: &mut Ctx<'_>, view: &mut View) {
    for (index, section) in page.sections().iter().enumerate() {
        if let Some(draw) = section.custom {
            ui.push_id(("drawn", index), |ui| draw(ui, c));
            continue;
        }
        let items: Vec<&Item> = section
            .items
            .iter()
            .filter(|item| item.applies(c))
            .collect();
        if items.is_empty() {
            continue;
        }
        match section.heading {
            Some(heading) => prefs::section(ui, tr(heading)),
            None => ui.add_space(10.0),
        }
        let enabled = section.enabled.is_none_or(|enabled| enabled(c));
        ui.add_enabled_ui(enabled, |ui| {
            prefs::card(ui, index, |card| {
                for item in items {
                    draw_item(card, page, item, c, view);
                }
            });
        });
        if let Some(footer) = section.footer {
            prefs::footer(ui, tr(footer));
        }
    }
}

/// Settings, as a sidebar of categories and a page of cards.
#[allow(clippy::too_many_arguments)]
pub fn settings_dialog(
    ctx: &Context,
    settings: &mut Settings,
    themes: &mut Vec<Theme>,
    macro_groups: &mut Vec<MacroGroup>,
    state: &mut PanelState,
    instances: &[String],
    servers: &ServerList,
    placement: &mut crate::ui::detach::Placement,
    buttons: &crate::config::theme::WindowButtons,
) -> Vec<UiRequest> {
    if !state.show_settings {
        return Vec::new();
    }
    let mut open = true;
    let mut c = Ctx {
        settings,
        themes,
        macro_groups,
        state,
        instances,
        servers,
        requests: Vec::new(),
        changed: false,
        theme_edited: false,
        go: None,
        index: 0,
    };
    // Before anything is drawn, so no page is drawn from a stale index or
    // misses the file a dialog has just picked.
    macros::forget_stale_selection(&mut c);
    screensaver::collect_picked_image(ctx, &mut c);
    crate::ui::detach::shell(
        ctx,
        "nit-settings",
        tr("Settings"),
        &mut open,
        [820.0, 640.0],
        buttons,
        // A window of its own, so it reopens where and how it was left for
        // whichever of the two switches on the Windows page is on.
        Some(placement),
        |ui| window(ui, &mut c),
    );
    if !open {
        c.state.show_settings = false;
    }
    // After the page, which is what moves the route, and on the frame the
    // window closes: an edit is kept by leaving it, as on every other page
    // here, since there is no OK to press.
    themes::settle(ctx, &mut c, open);
    macros::settle(&mut c, open);
    if !open || c.state.settings_route != MACROS_PAGE {
        // A picker left listening on a page nobody can see would keep the
        // app from answering its own shortcuts.
        c.state.capture_manager_shortcut = false;
    }
    if c.changed {
        c.requests.push(UiRequest::SettingsChanged);
    }
    c.requests
}

impl PanelState {
    /// Opens Settings on `route`, as every way into a manager now does.
    ///
    /// A search left typed would cover the page asked for with its results,
    /// so it goes.
    pub fn open_settings(&mut self, route: Route) {
        self.show_settings = true;
        self.settings_route = route;
        self.settings_search.clear();
    }
}

fn window(ui: &mut Ui, c: &mut Ctx<'_>) {
    let search_id = egui::Id::new("nit-settings-search");
    // Ctrl+F is the transcript's search in the main window; in this one there
    // is no transcript, and the settings search is the only thing to find in.
    let find =
        ui.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::F)));
    if find {
        ui.memory_mut(|m| m.request_focus(search_id));
    }
    let escape = ui.input(|i| i.key_pressed(Key::Escape));
    // Checked before anything is drawn, while a field that Esc is about to
    // take the keyboard from still has it: Esc in a field leaves the field,
    // and must not also leave the page.
    let nothing_focused = ui.memory(|m| m.focused().is_none());
    let back_button = ui.input(|i| i.pointer.button_pressed(PointerButton::Extra1));
    // Nor while a shortcut picker is listening, where Esc is how the
    // listening is called off.
    let capturing = c.state.macros.capture_shortcut || c.state.capture_manager_shortcut;
    let escape_back = escape && nothing_focused && !capturing;
    let back = find_page(c.state.settings_route).and_then(Page::back);
    if escape && !c.state.settings_search.is_empty() {
        c.state.settings_search.clear();
    } else if let Some(back) = back.filter(|_| escape_back || back_button) {
        c.go = Some(back);
    }

    let full = ui.available_rect_before_wrap();
    let side_width = (full.width() * 0.3).clamp(180.0, 240.0);
    let side = egui::Rect::from_min_size(full.min, egui::vec2(side_width, full.height()));
    let content = egui::Rect::from_min_max(egui::pos2(side.right() + 16.0, full.top()), full.max);

    let items: Vec<prefs::SidebarItem<'_>> = Category::ALL
        .iter()
        .map(|category| prefs::SidebarItem {
            label: tr(category.label()),
            symbol: category.symbol(),
            tile: category.tile(),
            group: category.starts_group(),
        })
        .collect();
    let searching = !c.state.settings_search.trim().is_empty();
    let selected = (!searching).then(|| c.state.settings_route.category.index());
    let mut side_ui =
        ui.child_ui_with_id_source(side, egui::Layout::top_down(egui::Align::Min), "side", None);
    let out = prefs::sidebar(
        &mut side_ui,
        search_id,
        &mut c.state.settings_search,
        tr("Search"),
        &items,
        selected,
    );
    if let Some(index) = out.clicked {
        c.go = Some(Route::page(Category::from_index(index)));
    }

    let now = ui.input(|i| i.time);
    let highlight = c.state.settings_highlight.and_then(|(key, since)| {
        let left = 1.0 - (now - since) / HIGHLIGHT_SECS;
        (left > 0.0).then_some((key, left as f32))
    });
    if highlight.is_some() {
        ui.ctx().request_repaint();
    } else {
        c.state.settings_highlight = None;
    }
    let query = c.state.settings_search.trim().to_owned();
    let mut view = View {
        highlight,
        scroll_to: c.state.settings_scroll_to.take(),
        results: !query.is_empty(),
        lead_to: None,
    };

    let mut content_ui = ui.child_ui_with_id_source(
        content,
        egui::Layout::top_down(egui::Align::Min),
        "content",
        None,
    );
    if query.is_empty() {
        page_view(&mut content_ui, c, &mut view);
    } else {
        results_view(&mut content_ui, c, &mut view, &query);
    }
    ui.allocate_rect(full, egui::Sense::hover());

    if let Some(route) = c.go.take() {
        c.state.settings_route = route;
        // Going anywhere is the end of a search: the results were the way
        // there, and leaving them up would hide the page it led to.
        c.state.settings_search.clear();
        if let Some(key) = view.lead_to {
            c.state.settings_highlight = Some((key, now));
            c.state.settings_scroll_to = Some(key);
        }
    }
}

fn page_view(ui: &mut Ui, c: &mut Ctx<'_>, view: &mut View) {
    let route = c.state.settings_route;
    // A sub-page that is gone - the profile it showed was deleted - falls
    // back to its category's page rather than leaving the window blank.
    let Some(page) = find_page(route).or_else(|| find_page(Route::page(route.category))) else {
        return;
    };
    c.index = route.index;
    let title = match page.heading {
        Some(heading) => heading(c),
        None => tr(page.title).to_owned(),
    };
    // Named for where it goes, which for a sub-page of a sub-page is the one
    // it was opened from - a theme by its name - rather than the category.
    let back = page.back();
    let back_label = back.and_then(find_page).map(|to| match to.heading {
        Some(heading) => heading(c),
        None => tr(to.title).to_owned(),
    });
    let went_back = prefs::column(ui, COLUMN, |ui| {
        prefs::page_header_with_back(ui, &title, back_label.as_deref())
    });
    if went_back {
        c.go = back;
    }
    if let Some(top) = page.top {
        prefs::column(ui, COLUMN, |ui| top(ui, c));
    }
    match page.body {
        Body::Sections(_) => {
            prefs::scroll_column(
                ui,
                ("nit-settings-page", route.category, route.sub),
                COLUMN,
                |ui| {
                    draw_sections(ui, page, c, view);
                },
            );
        }
        Body::Custom(draw) => draw(ui, c),
    }
}

fn results_view(ui: &mut Ui, c: &mut Ctx<'_>, view: &mut View, query: &str) {
    prefs::column(ui, COLUMN, |ui| {
        prefs::page_header(ui, &tr1("Results for \u{201c}{}\u{201d}", query));
    });
    prefs::scroll_column(ui, "nit-settings-results", COLUMN, |ui| {
        let mut found = false;
        for (page, hits) in search(query, tr) {
            let hits: Vec<&Item> = hits.into_iter().filter(|item| item.applies(c)).collect();
            if hits.is_empty() {
                continue;
            }
            found = true;
            if prefs::section_link(ui, &page.breadcrumb(tr)).clicked() {
                c.go = Some(page.route());
            }
            prefs::card(ui, ("result", page.category, page.sub), |card| {
                for item in hits {
                    draw_item(card, page, item, c, view);
                }
            });
        }
        if !found {
            prefs::empty_state(ui, tr("No settings match."));
        }
    });
}

/// Every page and sub-page, every section, every row - the one list the
/// window and its search both draw from.
fn pages() -> &'static [Page] {
    static PAGES: OnceLock<Vec<Page>> = OnceLock::new();
    PAGES.get_or_init(build_pages)
}

fn build_pages() -> Vec<Page> {
    let mut pages = vec![
        page(Category::General, general()),
        page(Category::Appearance, appearance()),
    ];
    pages.extend(themes::pages());
    pages.push(screensaver::page());
    pages.extend(vec![
        page(Category::Windows, windows()),
        subpage(
            Category::Windows,
            "size_presets",
            "Size presets",
            size_presets(),
        ),
        page(Category::Terminal, terminal()),
        page(Category::Keyboard, keyboard()),
    ]);
    pages.extend(macros::pages());
    pages.extend(vec![
        page(Category::Sessions, sessions()),
        Page {
            category: Category::Sessions,
            sub: Some("profile"),
            title: "Profile",
            heading: Some(profile_title),
            parent: None,
            top: None,
            body: Body::Custom(profile_page),
        },
        subpage(Category::Sessions, "shells", "Shells", shells()),
        page(Category::About, about()),
        subpage(Category::About, "proxy", "Proxy", proxy()),
    ]);
    pages
}

// ---------------------------------------------------------------------------
// General
// ---------------------------------------------------------------------------

fn general() -> Vec<Section> {
    vec![
        section(
            "Language",
            vec![Item::control("language", "Language", language)
                .keys(&["idioma", "english", "portugues", "translation", "traducao"])],
        ),
        section(
            "Status messages",
            vec![Item::control("status_timeout", "Hide status messages after", status_timeout)
                .sub("Seconds before a message in the footer goes away on its own. 0 leaves it until it is dismissed.")
                .keys(&["status", "footer", "message", "notification", "rodape", "mensagem", "aviso"])],
        ),
    ]
}

fn language(ui: &mut Ui, c: &mut Ctx<'_>) {
    prefs::popup(ui, "language-picker", c.settings.language.label(), |ui| {
        for lang in Lang::ALL {
            if ui
                .selectable_label(c.settings.language == lang, lang.label())
                .clicked()
            {
                c.settings.language = lang;
                // Applied at once rather than at the next start: the rest of
                // this window is what anyone changing the language wants to
                // read in it.
                crate::i18n::set_language(lang);
                c.changed = true;
            }
        }
    });
}

fn status_timeout(ui: &mut Ui, c: &mut Ctx<'_>) {
    c.changed |= ui
        .add(
            egui::DragValue::new(&mut c.settings.status_timeout_secs)
                .range(0..=600)
                .suffix(" s"),
        )
        .changed();
}

// ---------------------------------------------------------------------------
// Appearance
// ---------------------------------------------------------------------------

fn appearance() -> Vec<Section> {
    vec![
        section(
            "Theme",
            vec![
                Item::control("theme", "Theme", theme)
                    .keys(&["colour", "color", "cor", "cores", "dark", "light", "escuro", "claro"]),
                Item::link("theme_link", "Themes", Route::page(Category::Themes))
                    .sub("Duplicate a built-in theme and change any of its colours, including the ObjectScript ones.")
                    .keys(&["edit", "editar", "manager", "gerenciador"]),
            ],
        ),
        section(
            "Font",
            vec![
                Item::control("font", "Font", font)
                    .sub("Monospace families only: the terminal is a character grid, so a proportional font would not line up.")
                    .keys(&["typeface", "monospace", "fonte", "letra"]),
                Item::control("font_size", "Font size", font_size)
                    .keys(&["size", "tamanho", "zoom"]),
            ],
        ),
        section(
            "Cursor",
            vec![
                Item::control("cursor_style", "Cursor", cursor_style)
                    .keys(&["block", "bar", "underscore", "caret", "bloco", "barra"]),
                Item::toggle("cursor_blink", "Blink", |s| &mut s.cursor_blink)
                    .keys(&["cursor", "blink", "piscar"]),
            ],
        ),
        section(
            "Highlighting",
            vec![
                Item::toggle("syntax_highlight", "Syntax highlighting", |s| {
                    &mut s.terminal_syntax_highlight
                })
                .hint("Colours globals, strings, numbers, commands, macros and class references. A guess about the text on screen; a colour IRIS sets itself always wins.")
                .keys(&["colour", "color", "objectscript", "cores", "sintaxe", "realce"]),
                Item::toggle("sql_highlight", "Colour SQL at the SQL shell's prompt", |s| {
                    &mut s.sql_highlight
                })
                .keys(&["sql", "colour", "color", "cores"]),
            ],
        ),
        section(
            "Interface",
            vec![
                Item::control("ui_scale", "Interface scale", ui_scale)
                    .sub("Enlarges the tabs, the title bar, the dialogs and the managers. The terminal keeps the font size above.")
                    .keys(&["zoom", "dpi", "size", "tamanho", "escala"]),
                Item::control("title_bar_scale", "Title bar scale", title_bar_scale)
                    .sub("Enlarges the title bar and the tabs again, on top of the interface scale, leaving the dialogs and the managers as they are.")
                    .keys(&["zoom", "tabs", "abas", "size", "tamanho", "escala", "title bar"]),
                Item::toggle("show_scrollbars", "Show scrollbars", |s| &mut s.show_scrollbars)
                    .sub("Solid scrollbars instead of the thin ones that only appear on hover.")
                    .keys(&["scrollbar", "barra de rolagem"]),
            ],
        ),
    ]
}

fn theme(ui: &mut Ui, c: &mut Ctx<'_>) {
    let themes = &*c.themes;
    prefs::popup(ui, "theme-picker", c.settings.theme.clone(), |ui| {
        for theme in themes {
            if ui
                .selectable_label(theme.name == c.settings.theme, &theme.name)
                .clicked()
            {
                c.settings.theme = theme.name.clone();
                c.changed = true;
            }
        }
    });
}

fn font(ui: &mut Ui, c: &mut Ctx<'_>) {
    let families = c
        .state
        .font_families
        .get_or_insert_with(crate::ui::fonts::monospace_families);
    let selected = if c.settings.font_family.is_empty() {
        tr(BUILT_IN_FONT).to_owned()
    } else {
        c.settings.font_family.clone()
    };
    let settings = &mut *c.settings;
    let mut changed = false;
    prefs::popup(ui, "font-picker", selected, |ui| {
        if ui
            .selectable_label(settings.font_family.is_empty(), tr(BUILT_IN_FONT))
            .clicked()
        {
            settings.font_family.clear();
            changed = true;
        }
        for family in families.iter() {
            if ui
                .selectable_label(&settings.font_family == family, family)
                .clicked()
            {
                settings.font_family = family.clone();
                changed = true;
            }
        }
    });
    c.changed |= changed;
}

fn font_size(ui: &mut Ui, c: &mut Ctx<'_>) {
    c.changed |= prefs::slider(ui, &mut c.settings.font_size, 8.0..=28.0, None, |s| s).changed();
}

fn cursor_style(ui: &mut Ui, c: &mut Ctx<'_>) {
    let options: Vec<(CursorStyle, &str)> = CursorStyle::ALL
        .iter()
        .map(|&style| (style, tr(style.label())))
        .collect();
    c.changed |= prefs::segmented(ui, &mut c.settings.cursor_style, &options);
}

/// The interface scale's steps, in percent.
const UI_SCALE_STEPS: [u32; 5] = [100, 125, 150, 175, 200];

/// Fixed steps plus a typed percentage, never a slider: the scale applies
/// while it is being changed, so a dragged slider grew under the pointer and
/// ran away from it. Clicking a step or typing a number moves nothing until
/// it is done. The field does not drag either, for the same reason; it only
/// takes what is typed into it.
fn ui_scale(ui: &mut Ui, c: &mut Ctx<'_>) {
    scale_picker(ui, c, |s| &mut s.ui_scale);
}

fn title_bar_scale(ui: &mut Ui, c: &mut Ctx<'_>) {
    scale_picker(ui, c, |s| &mut s.title_bar_scale);
}

/// The steps, and a field to type any percentage between them.
fn scale_picker(ui: &mut Ui, c: &mut Ctx<'_>, field: fn(&mut Settings) -> &mut f32) {
    let mut percent = (*field(c.settings) * 100.0).round() as u32;
    // Laid out right to left: the field ends up at the right edge.
    let typed = ui
        .add(
            egui::DragValue::new(&mut percent)
                .speed(0.0)
                .range(100..=200)
                .suffix(" %"),
        )
        // A field to type in, not to drag: the speed is zero, so the
        // sideways arrows a drag value shows on hover promised a gesture
        // that did nothing.
        .on_hover_cursor(egui::CursorIcon::Text)
        .on_hover_text(tr("Type a percentage from 100 to 200."))
        .changed();
    let mut picked = None;
    for step in UI_SCALE_STEPS.iter().rev() {
        if ui
            .selectable_label(percent == *step, format!("{step}%"))
            .clicked()
        {
            picked = Some(*step);
        }
    }
    if let Some(step) = picked.or(typed.then_some(percent)) {
        let scale = step.clamp(100, 200) as f32 / 100.0;
        let value = field(c.settings);
        if scale != *value {
            *value = scale;
            c.changed = true;
        }
    }
}

// ---------------------------------------------------------------------------
// Window and tabs
// ---------------------------------------------------------------------------

/// The title bar and the tabs, ahead of the window's size and behaviour: one
/// page for the whole window, rather than two that split it at the frame.
fn title_bar() -> Vec<Section> {
    vec![
        section(
            "Title bar",
            vec![
                Item::toggle("tabs_in_title_bar", "Tabs on window title bar", |s| {
                    &mut s.tabs_in_title_bar
                })
                .hint("Puts the tabs on the same row as the window buttons, from the new-session button across to the gear. One row instead of two; the session line - instance, PID and size - goes, since the tabs already say which session it is.")
                .keys(&["tabs", "abas"]),
            ],
        ),
        section(
            "Information shown",
            vec![
                Item::toggle("show_namespace_in_tab", "Show the namespace in the tab name", |s| {
                    &mut s.show_namespace_in_tab
                })
                .hint("Adds the namespace the session is in to the tab's name - CONSISTEM | RDB76-TR. Read off the prompt, so it follows a ZN as it happens; a tab renamed by hand keeps the name it was given.")
                .keys(&["namespace", "tab", "aba"]),
                // Shown on the session line, which a title bar holding the
                // tabs does not have: the switch would do nothing there.
                Item::toggle("show_pid", "Show the process id", |s| &mut s.show_pid)
                    .when(|c| !c.settings.tabs_in_title_bar)
                    .hint("Puts the session's process id next to the instance name and the window size in the menu bar. A local session only: a remote one runs its process on the far side.")
                    .keys(&["pid", "process", "processo"]),
            ],
        ),
    ]
}

// ---------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------

fn windows() -> Vec<Section> {
    let mut sections = title_bar();
    sections.extend(vec![
        section(
            "Terminal size",
            vec![
                Item::control("default_cols", "Columns", default_cols)
                    .keys(&["width", "largura", "size", "tamanho"]),
                Item::control("default_rows", "Rows", default_rows)
                    .keys(&["height", "altura", "size", "tamanho", "linhas"]),
                Item::link(
                    "size_presets_link",
                    "Size presets",
                    Route::sub(Category::Windows, "size_presets"),
                )
                .keys(&["80", "132", "common", "comum"]),
            ],
        )
        .footer("The size a window opens at when it is not reopening at the last one. In characters, so it holds the same amount of output at any font size."),
        section(
            "Size and position",
            vec![
                Item::toggle("save_terminal_size", "Save terminal size", |s| {
                    &mut s.save_terminal_size
                })
                .hint("Reopens the window at the size it was last closed at. Off opens it at 100x30 characters, whatever the font size.")
                .keys(&["remember", "lembrar", "salvar"]),
                Item::toggle("save_window_position", "Save window position", |s| {
                    &mut s.save_window_position
                })
                .hint("Reopens the window where it was last closed. Off centres it on the screen.")
                .keys(&["remember", "lembrar", "salvar", "posicao"]),
            ],
        )
        .footer("Takes effect the next time the app starts. The Settings window always reopens where it was left."),
        section(
            "Desktop",
            vec![Item::toggle("pin_to_desktop", "Pin to desktop", |s| &mut s.pin_to_desktop)
                .hint("Keeps the window on screen when the desktop is shown (Win+D). Otherwise it is an ordinary window: others cover it, and clicking it brings it to the front.")
                .keys(&["win+d", "desktop", "fixar", "area de trabalho"])],
        ),
        section(
            "Closing",
            vec![
                Item::toggle(
                    "confirm_close",
                    "Ask before closing with a session still connected",
                    |s| &mut s.confirm_close_with_live_session,
                )
                .keys(&["confirm", "quit", "exit", "confirmar", "sair"]),
                Item::toggle("close_to_tray", "Close to the tray", |s| &mut s.close_to_tray)
                    .hint("Closing the window hides it behind an icon in the notification area, and the sessions stay connected. Click the icon to bring it back; its menu's Exit quits.")
                    .keys(&["tray", "notification", "bandeja", "notificacao", "minimize"]),
            ],
        ),
    ]);
    sections
}

fn size_presets() -> Vec<Section> {
    vec![untitled(vec![
        Item::control("cols_presets", "Columns", cols_presets)
            .keys(&["width", "largura", "80", "100", "132"]),
        Item::control("rows_presets", "Rows", rows_presets)
            .keys(&["height", "altura", "linhas", "24", "30", "48"]),
    ])
    .footer("The sizes terminals have traditionally come in. Picking one sets the size a window opens at.")]
}

fn default_cols(ui: &mut Ui, c: &mut Ctx<'_>) {
    c.changed |= ui
        .add(egui::DragValue::new(&mut c.settings.default_cols).range(20..=500))
        .changed();
}

fn default_rows(ui: &mut Ui, c: &mut Ctx<'_>) {
    c.changed |= ui
        .add(egui::DragValue::new(&mut c.settings.default_rows).range(5..=200))
        .changed();
}

/// One of the common sizes, as a segmented control. A size typed by hand that
/// is none of them leaves every segment unlit, which is the truth.
fn presets(ui: &mut Ui, value: &mut u16, common: [u16; 3]) -> bool {
    let labels: Vec<String> = common.iter().map(u16::to_string).collect();
    let options: Vec<(u16, &str)> = common
        .iter()
        .zip(&labels)
        .map(|(&n, label)| (n, label.as_str()))
        .collect();
    prefs::segmented(ui, value, &options)
}

fn cols_presets(ui: &mut Ui, c: &mut Ctx<'_>) {
    c.changed |= presets(ui, &mut c.settings.default_cols, crate::config::COMMON_COLS);
}

fn rows_presets(ui: &mut Ui, c: &mut Ctx<'_>) {
    c.changed |= presets(ui, &mut c.settings.default_rows, crate::config::COMMON_ROWS);
}

// ---------------------------------------------------------------------------
// Terminal
// ---------------------------------------------------------------------------

fn terminal() -> Vec<Section> {
    vec![
        section(
            "Scrollback",
            vec![
                Item::control("scrollback_limit", "Scrollback lines", scrollback_limit)
                    .keys(&["history", "buffer", "lines", "historico", "linhas"]),
                Item::toggle("wrap_lines", "Wrap long lines", |s| &mut s.wrap_lines)
                    .hint("On: a long line continues on the next row, breaking at the window edge. Off: it runs off to the right, reached by scrolling sideways or widening the window.")
                    .keys(&["wrap", "quebra", "linha", "horizontal"]),
            ],
        )
        .footer("Either way the whole line is kept: the terminal is reported wider than the window, because IRIS cuts a line at the terminal width instead of wrapping it."),
        section(
            "Global tooltip",
            vec![Item::control("intellisense", "Global tooltip", intellisense)
                .hint("What a piece or a subscript of a zwrite'n global means, read out of the class that maps it. On selection: only over text you have selected, which is what a double-click on a piece already gives. On hover: over whatever the pointer is on, with nothing selected. Off: never asked, and no second session is opened to ask with.")
                .keys(&["intellisense", "piece", "subscript", "zwrite", "dica", "global"])],
        ),
    ]
}

fn scrollback_limit(ui: &mut Ui, c: &mut Ctx<'_>) {
    c.changed |= ui
        .add(egui::DragValue::new(&mut c.settings.scrollback_limit).range(0..=200_000))
        .changed();
}

fn intellisense(ui: &mut Ui, c: &mut Ctx<'_>) {
    let options: Vec<(IntellisenseMode, &str)> = IntellisenseMode::ALL
        .iter()
        .map(|&mode| (mode, tr(mode.label())))
        .collect();
    c.changed |= prefs::segmented(ui, &mut c.settings.intellisense, &options);
}

// ---------------------------------------------------------------------------
// Keyboard and editing
// ---------------------------------------------------------------------------

fn keyboard() -> Vec<Section> {
    vec![
        section(
            "Editing",
            vec![
                Item::toggle("copy_on_select", "Copy on select", |s| &mut s.copy_on_select)
                    .sub("Put a selection on the clipboard as soon as the mouse is released, without waiting for Ctrl+C.")
                    .keys(&["clipboard", "copy", "copiar", "selecao", "area de transferencia"]),
                Item::toggle("surround_selection", "Quotes and brackets wrap the selection", |s| {
                    &mut s.surround_selection
                })
                .hint("On: typing \" ' ( [ or { over selected text on the command line puts the pair around it instead of replacing it, the way an editor does - so selecting a global name and pressing \" quotes it, and the text stays selected to be wrapped again. Off: the character replaces the selection. Only applies to a selection inside the line being typed; one in the scrollback is highlighted text and is never edited.")
                .keys(&["quote", "bracket", "surround", "aspas", "parenteses"]),
                Item::toggle("autocomplete", "Autocomplete", |s| &mut s.autocomplete)
                    .sub("Offers the rest of the word being typed at an IRIS prompt. Tab accepts, Esc closes.")
                    .keys(&["completion", "intellisense", "suggest", "sugestao", "completar"]),
            ],
        ),
        section(
            "Command history",
            vec![
                Item::toggle("recall_mid_line", "Up and Down recall from anywhere on the line", |s| {
                    &mut s.recall_mid_line
                })
                .hint("On: Up replaces the line with an earlier command wherever the cursor is, the way the native IRIS terminal does. Off: only at the end of the line, so a cursor left in the middle means the line is being edited and the arrows leave it alone.")
                .keys(&["arrow", "history", "recall", "seta", "historico"]),
                Item::toggle("save_command_history", "Remember commands from earlier sessions", |s| {
                    &mut s.save_command_history
                })
                .hint("Keeps the commands typed at an IRIS prompt in history.txt, so Up reaches back past the sessions open now. Off keeps recall working inside each session and writes nothing to disk. Either way a tab offers back its own commands first and the inherited ones after them, and lines sent by a macro or an IRIS helper are never offered back at all.")
                .keys(&["history", "recall", "historico", "comandos"]),
            ],
        ),
    ]
}

// ---------------------------------------------------------------------------
// Sessions
// ---------------------------------------------------------------------------

fn sessions() -> Vec<Section> {
    vec![
        section(
            "Profiles",
            vec![
                Item::rows("profiles", "Profiles", profile_rows).keys(&[
                    "instance", "server", "telnet", "encoding", "connection", "instancia",
                    "servidor", "codificacao", "conexao", "perfil",
                ]),
                Item::control("default_profile", "Default profile", default_profile)
                    .when(|c| !c.settings.profiles.is_empty())
                    .keys(&["startup", "inicializacao", "padrao", "perfil"]),
            ],
        ),
        section(
            "Startup",
            vec![
                Item::toggle("open_on_start", "Open the default profile at startup", |s| {
                    &mut s.open_on_start
                })
                .keys(&["start", "launch", "iniciar", "abrir"]),
                Item::toggle("remember_open_tabs", "Remember open tabs and namespaces", |s| {
                    &mut s.remember_open_tabs
                })
                .hint("Reopens the tabs and splits that were open when the terminal was closed, instead of the default profile, and takes each IRIS session back to the namespace it was left in. The sessions themselves are new: what was on screen is shown above them, but variables, locks and routines in progress are not. Closing every tab before closing the window leaves nothing to reopen.")
                .keys(&["restore", "session", "restaurar", "sessao", "abas"]),
            ],
        ),
        section(
            "Logging",
            vec![
                Item::control("default_log_mode", "Default mode", default_log_mode)
                    .keys(&["transcript", "log", "raw", "clean", "transcricao", "bruto"]),
                Item::control("log_retention_days", "Keep logs for (days, 0 = forever)", log_retention)
                    .keys(&["retention", "days", "delete", "retencao", "dias", "apagar"]),
                Item::rows("log_dir", "Logs folder", log_dir)
                    .keys(&["folder", "path", "pasta", "caminho"]),
            ],
        ),
        section(
            "Other shells",
            vec![Item::link("shells_link", "Shells", Route::sub(Category::Sessions, "shells"))
                .sub("Other command interpreters, offered under Shells in the new-session menu.")
                .keys(&["cmd", "powershell", "bash", "wsl", "git", "interpreter", "interpretador"])],
        ),
    ]
}

/// One row per profile, leading to its own page, and the button that adds
/// another.
///
/// A page per profile rather than every field of every profile in one list:
/// a profile is several fields, and a list of names is what someone looking
/// for one of them needs to see first.
fn profile_rows(card: &mut Card<'_>, c: &mut Ctx<'_>) {
    for (index, profile) in c.settings.profiles.iter().enumerate() {
        let name = if profile.name.is_empty() {
            tr1("Profile {}", &(index + 1).to_string())
        } else {
            profile.name.clone()
        };
        let mut subtitle = instance_label(profile);
        if profile.name == c.settings.default_profile {
            subtitle = format!("{subtitle} - {}", tr("Default"));
        }
        if card
            .nav(Row::new(&name).subtitle(subtitle.as_str()))
            .clicked()
        {
            c.go = Some(Route {
                index,
                ..Route::sub(Category::Sessions, "profile")
            });
        }
    }
    card.buttons(|ui| {
        if prefs::button(ui, tr("Add profile")).clicked() {
            c.settings.profiles.push(Profile {
                name: format!("Profile {}", c.settings.profiles.len() + 1),
                instance: c.instances.first().cloned().unwrap_or_default(),
                ..Profile::default()
            });
            c.changed = true;
            // Straight onto its page: a new profile is added to be filled in.
            c.go = Some(Route {
                index: c.settings.profiles.len() - 1,
                ..Route::sub(Category::Sessions, "profile")
            });
        }
    });
}

fn profile_title(c: &Ctx<'_>) -> String {
    match c.settings.profiles.get(c.index) {
        Some(profile) if !profile.name.is_empty() => profile.name.clone(),
        _ => tr1("Profile {}", &(c.index + 1).to_string()),
    }
}

/// One profile's page: where it connects, how, and what it logs.
fn profile_page(ui: &mut Ui, c: &mut Ctx<'_>) {
    let index = c.index;
    if index >= c.settings.profiles.len() {
        c.go = Some(Route::page(Category::Sessions));
        return;
    }
    let instances = c.instances;
    let servers = c.servers;
    let mut delete = false;
    prefs::scroll_column(ui, ("nit-settings-profile", index), COLUMN, |ui| {
        let profile = &mut c.settings.profiles[index];
        ui.add_space(10.0);
        let changed = prefs::card(ui, "connection", |card| {
            profile_editor(card, index, profile, instances, servers)
        });
        c.changed |= changed;
        ui.add_space(14.0);
        prefs::card(ui, "delete", |card| {
            card.buttons(|ui| {
                if prefs::button(ui, tr("Delete this profile")).clicked() {
                    delete = true;
                }
            });
        });
    });
    if delete {
        // Forget the stored secret too, or it outlives the profile that
        // explained what it was for.
        c.settings.profiles[index].clear_password();
        c.settings.profiles.remove(index);
        c.changed = true;
        c.go = Some(Route::page(Category::Sessions));
    }
}

/// One profile's fields, as rows. Returns true when any of them changed.
fn profile_editor(
    card: &mut Card<'_>,
    index: usize,
    profile: &mut Profile,
    instances: &[String],
    servers: &ServerList,
) -> bool {
    let mut changed = false;
    card.row(Row::new(tr("Name")), |ui| {
        changed |= ui
            .add(egui::TextEdit::singleline(&mut profile.name).desired_width(220.0))
            .changed();
    });

    // Where the session actually goes, since a server entry can be a Telnet
    // login rather than an instance on this machine.
    let target = profile.remote.as_ref().map(|remote| {
        tr1(
            "Telnet login to {}",
            &format!("{}:{}", remote.address, remote.port),
        )
    });
    card.row(Row::new(tr("Instance")).subtitle(target.as_deref()), |ui| {
        if servers.is_empty() && instances.is_empty() {
            // Nothing was discovered - no launcher, or a machine this app
            // cannot read the list on - so the name is typed.
            changed |= ui
                .add(egui::TextEdit::singleline(&mut profile.instance).desired_width(220.0))
                .changed();
        } else {
            let label = instance_label(profile);
            if let Some(picked) = prefs::popup(ui, ("instance", index), label, |ui| {
                instance_menu(ui, profile, instances, servers)
            }) {
                changed |= picked;
            }
        }
    });

    // Only a Telnet session has a charset to choose. A local one runs inside a
    // pseudo-console, which hands the terminal UTF-8 whatever codepage it is
    // on, so there is nothing here that could change it - offering the choice
    // only invited a setting that costs a column per accent. See
    // `Profile::wire_encoding`.
    if profile.remote.is_some() {
        let spec = Row::new(tr("Encoding")).subtitle(tr(
            "Leave as UTF-8 unless accented characters come out wrong.",
        ));
        card.row(spec, |ui| {
            prefs::popup(
                ui,
                ("encoding", index),
                tr(profile.encoding.label()),
                |ui| {
                    for enc in Encoding::ALL {
                        if ui
                            .selectable_label(profile.encoding == enc, tr(enc.label()))
                            .clicked()
                        {
                            profile.encoding = enc;
                            changed = true;
                        }
                    }
                },
            );
        });
    } else {
        let spec = Row::new(tr("Encoding")).subtitle(tr("A local session speaks UTF-8."));
        card.row(spec, |ui| {
            ui.weak("UTF-8");
        });
    }

    card.row(Row::new(tr("Logging")), |ui| {
        changed |= prefs::segmented(
            ui,
            &mut profile.logging,
            &[
                (LogMode::Off, tr("Use default")),
                (LogMode::Clean, tr("Clean")),
                (LogMode::Raw, tr("Raw")),
            ],
        );
    });
    changed
}

fn default_profile(ui: &mut Ui, c: &mut Ctx<'_>) {
    let settings = &mut *c.settings;
    let mut changed = false;
    prefs::popup(
        ui,
        "default-profile",
        settings.default_profile.clone(),
        |ui| {
            for profile in &settings.profiles {
                if ui
                    .selectable_label(profile.name == settings.default_profile, &profile.name)
                    .clicked()
                {
                    settings.default_profile = profile.name.clone();
                    changed = true;
                }
            }
        },
    );
    c.changed |= changed;
}

/// What the instance picker shows when it is closed.
///
/// The endpoint rather than the bare name when the two differ: a profile
/// pointing at a remote server names the server in `instance`, and the address
/// is the part that says it is not the local instance of the same name.
fn instance_label(profile: &Profile) -> String {
    match profile.remote.as_ref() {
        Some(remote) => format!("{}  ({})", profile.instance, remote.address),
        None if profile.instance.is_empty() => tr("Choose...").to_string(),
        None => profile.instance.clone(),
    }
}

/// The instance picker's contents: the same list the `+` button's right-click
/// menu offers, because it is the same question - which server or instance does
/// this profile connect to.
///
/// Returns true when a choice was made.
fn instance_menu(
    ui: &mut Ui,
    profile: &mut Profile,
    instances: &[String],
    servers: &ServerList,
) -> bool {
    let mut changed = false;

    if !servers.is_empty() {
        ui.weak(tr("IRIS servers"));
        for server in &servers.servers {
            let selected = profile.instance.eq_ignore_ascii_case(&server.name);
            let entry = ui.selectable_label(selected, server.menu_label());
            // What the entry does, since a local server opens an instance and
            // a remote one is a Telnet login.
            let hint = match server.target(instances) {
                Target::Local { instance } => tr1("Local session on instance {}", &instance),
                Target::Telnet { address, port } => {
                    tr1("Telnet login to {}", &format!("{address}:{port}"))
                }
            };
            if entry.on_hover_text(hint).clicked() {
                point_at(profile, server, instances);
                changed = true;
            }
        }
        let loose = instances
            .iter()
            .any(|name| !servers.covers_instance(instances, name));
        if loose {
            ui.separator();
        }
    }

    for name in instances {
        // Skipped when a server entry already opens it: the same session under
        // two names is not a choice.
        if servers.covers_instance(instances, name) {
            continue;
        }
        let selected = profile.remote.is_none() && &profile.instance == name;
        if ui.selectable_label(selected, name).clicked() {
            point_at(profile, &Server::for_instance(name), instances);
            changed = true;
        }
    }

    changed
}

/// Points a profile at one of the launcher's servers, exactly as the `+` menu
/// does: the name is what the tab will say, and the target decides whether the
/// session starts locally or logs in over Telnet.
fn point_at(profile: &mut Profile, server: &Server, instances: &[String]) {
    match server.target(instances) {
        Target::Local { instance } => {
            profile.instance = instance;
            profile.remote = None;
        }
        Target::Telnet { address, port } => {
            profile.instance = server.name.clone();
            profile.remote = Some(Remote { address, port });
        }
    }
}

fn default_log_mode(ui: &mut Ui, c: &mut Ctx<'_>) {
    c.changed |= prefs::segmented(
        ui,
        &mut c.settings.default_log_mode,
        &[
            (LogMode::Off, tr("Off")),
            (LogMode::Clean, tr("Clean text")),
            (LogMode::Raw, tr("Raw bytes")),
        ],
    );
}

fn log_retention(ui: &mut Ui, c: &mut Ctx<'_>) {
    c.changed |= ui
        .add(egui::DragValue::new(&mut c.settings.log_retention_days).range(0..=3650))
        .changed();
}

/// A row of its own kind because its second line is the folder itself, which
/// a declaration made once cannot know.
fn log_dir(card: &mut Card<'_>, c: &mut Ctx<'_>) {
    let path = c.settings.log_dir.clone();
    let shown = path.display().to_string();
    card.row(Row::new(tr("Logs folder")).subtitle(shown.as_str()), |ui| {
        if prefs::button(ui, tr("Open folder")).clicked() {
            c.requests.push(UiRequest::OpenFolder(path.clone()));
        }
    });
}

fn shells() -> Vec<Section> {
    vec![
        untitled(vec![Item::rows("shells", "Shells", shell_rows)
            .keys(&["cmd", "powershell", "bash", "wsl", "git", "interpreter", "interpretador"])])
        .footer("Other command interpreters, offered under Shells in the new-session menu. Every one of them is a .toml file in the folder below - the ones found installed on this machine were written there for you, and can be renamed, re-armed or deleted like any other."),
        untitled(vec![Item::control("shells_folder", "Shells folder", shells_folder)
            .keys(&["folder", "toml", "reload", "pasta", "recarregar"])]),
    ]
}

/// One row per shell found. Nothing here is a setting: the files in the
/// folder are.
fn shell_rows(card: &mut Card<'_>, _: &mut Ctx<'_>) {
    let shells = crate::plugins::shells::available();
    if shells.is_empty() {
        card.row(Row::new(tr("None found.")), |_| ());
    }
    for shell in &shells {
        let command = shell.command_line();
        card.row(Row::new(&shell.name).subtitle(command.as_str()), |ui| {
            ui.weak(tr(shell.source_label()));
        });
    }
}

fn shells_folder(ui: &mut Ui, c: &mut Ctx<'_>) {
    if prefs::button(ui, tr("Open folder"))
        .on_hover_text(crate::plugins::shells::shells_dir().display().to_string())
        .clicked()
    {
        c.requests
            .push(UiRequest::OpenFolder(crate::plugins::shells::shells_dir()));
    }
    if prefs::button(ui, tr("Reload"))
        .on_hover_text(tr("Probe again and re-read the folder."))
        .clicked()
    {
        // The list is cached for the process, because the new-session menu
        // asks for it on every frame it is open. This is the way to say a
        // file has just changed.
        crate::plugins::shells::refresh();
    }
}

// ---------------------------------------------------------------------------
// About
// ---------------------------------------------------------------------------

/// What the terminal can do that a plain one cannot, and how to reach it.
///
/// The gestures are scattered - a shortcut here, a right-click entry there, a
/// word typed at the prompt - and none of them announces itself, so this is
/// where someone looking for them can find them all at once. Each is a row of
/// its own, so the search finds a tip by what it is about.
const FEATURE_TIPS: &[(&str, &str, &str)] = &[
    ("tip_sql", "SQL mode", "Type /sql at an IRIS prompt and press Enter, or press Ctrl+Shift+Q, or use the right-click menu. The prompt becomes NAMESPACE>> and lines are coloured as SQL; end a multi-line statement with GO. The same gesture leaves it."),
    ("tip_autocomplete", "Autocomplete", "Suggestions appear as you type at an IRIS prompt: commands, $functions, ^globals, routines, ##class( names and, at the SQL prompt, SQL keywords and tables. Up/Down choose, Tab accepts, Esc closes."),
    ("tip_recall", "Command recall", "Up and Down recall commands from this and earlier sessions, from anywhere on the line."),
    ("tip_globals", "Global tooltips", "Point at a global reference to see what its subscripts and pieces mean."),
    ("tip_macros", "Macros", "Right-click the terminal to run a macro. A {{name}} in its command is asked for before it runs."),
    ("tip_tabs", "Tabs", "Ctrl+T opens a session, Ctrl+Shift+T reopens the last one closed. Right-click a tab to split it; drag tabs to reorder; middle-click closes."),
    ("tip_find", "Find", "Ctrl+F searches the whole transcript."),
    ("tip_themes", "Themes", "The Themes page edits every colour of a theme. Point at a swatch and press Ctrl+C or Ctrl+V to copy a colour between elements; drag the title-bar rows to reorder the bar."),
];

fn about() -> Vec<Section> {
    let tips = FEATURE_TIPS
        .iter()
        .map(|&(key, title, text)| {
            Item::new(key, title, Kind::Note)
                .sub(text)
                .keys(&["tip", "dica", "help", "ajuda"])
        })
        .collect();
    vec![
        section(
            "Automatic updates",
            vec![
                Item::control("version", "Version", version)
                    .keys(&["update", "release", "versao", "atualizar", "atualizacao"]),
                Item::toggle("check_for_updates", "Check for a new version at startup", |s| {
                    &mut s.check_for_updates
                })
                .hint("One request to GitHub through the machine's own proxy. Nothing is downloaded or replaced without being asked.")
                .keys(&["update", "github", "atualizar", "atualizacao"]),
                Item::link("proxy_link", "Proxy", Route::sub(Category::About, "proxy"))
                    .keys(&["network", "http", "credentials", "rede", "credenciais"]),
            ],
        ),
        section("Did you know?", tips),
    ]
}

fn proxy() -> Vec<Section> {
    vec![untitled(vec![
        Item::rows("proxy", "Proxy", proxy_status).keys(&["network", "http", "rede", "conexao"]),
        Item::control("proxy_user", "Proxy user", proxy_user)
            .when(has_proxy)
            .keys(&["proxy", "user", "login", "usuario"]),
        Item::control("proxy_password", "Password", proxy_password)
            .when(has_proxy)
            .keys(&["proxy", "password", "credential", "senha", "credencial"]),
    ])
    .footer("Basic authentication only. A proxy that insists on NTLM cannot be reached this way; download the release from the browser instead.")]
}

fn version(ui: &mut Ui, c: &mut Ctx<'_>) {
    if prefs::button(ui, tr("Check now")).clicked() {
        c.requests.push(UiRequest::CheckForUpdates);
    }
    ui.weak(crate::features::update::CURRENT);
}

fn has_proxy(_: &Ctx<'_>) -> bool {
    crate::features::update::system_proxy().is_some()
}

fn proxy_status(card: &mut Card<'_>, _: &mut Ctx<'_>) {
    let status = match crate::features::update::system_proxy() {
        Some(proxy) => tr1("Going through the system proxy at {}.", &proxy),
        None => tr("No system proxy configured; connecting directly.").to_owned(),
    };
    card.row(Row::new(tr("Proxy")).subtitle(status.as_str()), |_| ());
}

/// Only shown when there is a proxy to authenticate to. A proxy that lets the
/// check through and then demands credentials for the host GitHub serves the
/// release from is what left this updater checking successfully and never
/// downloading, so the fields are here rather than the failure being something
/// only the log knows about.
fn proxy_user(ui: &mut Ui, c: &mut Ctx<'_>) {
    c.changed |= ui
        .add(egui::TextEdit::singleline(&mut c.settings.proxy_user).desired_width(180.0))
        .on_hover_text(tr(
            "Only if the proxy asks for credentials. Leave empty otherwise.",
        ))
        .changed();
}

fn proxy_password(ui: &mut Ui, c: &mut Ctx<'_>) {
    // Not read back out of the credential store to fill this in: a password
    // manager is not a place to show passwords from. Typing here replaces what
    // is stored, and emptying the field forgets it.
    if ui
        .add(
            egui::TextEdit::singleline(&mut c.state.proxy_password)
                .password(true)
                .hint_text(if crate::features::update::has_proxy_password() {
                    tr("stored")
                } else {
                    tr("none")
                })
                .desired_width(180.0),
        )
        .on_hover_text(tr(
            "Kept in the operating system's credential store, never in settings.toml.",
        ))
        .lost_focus()
    {
        c.requests
            .push(UiRequest::SetProxyPassword(c.state.proxy_password.clone()));
        c.state.proxy_password.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::translate;

    fn english(text: &'static str) -> &'static str {
        text
    }

    fn portuguese(text: &'static str) -> &'static str {
        translate(Lang::PtBr, text)
    }

    fn found(query: &str, tr: fn(&'static str) -> &'static str) -> Vec<&'static str> {
        search(query, tr)
            .into_iter()
            .flat_map(|(_, hits)| hits.into_iter().map(|item| item.key))
            .collect()
    }

    #[test]
    fn every_category_comes_back_from_its_own_position() {
        for (position, category) in Category::ALL.iter().enumerate() {
            assert_eq!(category.index(), position);
            assert_eq!(Category::from_index(position), *category);
        }
    }

    /// A position the sidebar cannot have produced still opens a page, rather
    /// than an empty window.
    #[test]
    fn a_position_past_the_end_opens_the_first_page() {
        assert_eq!(Category::from_index(99), Category::General);
    }

    #[test]
    fn every_category_has_exactly_one_page_of_its_own_and_it_is_not_empty() {
        for category in Category::ALL {
            let own: Vec<&Page> = pages()
                .iter()
                .filter(|p| p.category == category && p.sub.is_none())
                .collect();
            assert_eq!(own.len(), 1, "{category:?}");
            assert!(own[0].sections().iter().any(|s| !s.items.is_empty()));
        }
    }

    #[test]
    fn no_two_pages_share_a_route() {
        let routes: Vec<Route> = pages().iter().map(Page::route).collect();
        for (index, route) in routes.iter().enumerate() {
            assert!(
                !routes[..index].contains(route),
                "{route:?} is declared twice"
            );
        }
    }

    /// A row that leads nowhere would be a chevron that does nothing.
    #[test]
    fn every_link_leads_to_a_page_that_exists() {
        for page in pages() {
            for section in page.sections() {
                for item in &section.items {
                    if let Kind::Link(to) = item.kind {
                        assert!(find_page(to).is_some(), "{} leads to {to:?}", item.key);
                    }
                }
            }
        }
    }

    /// Every sub-page is reached from somewhere: one with no way in is
    /// settings nobody can get to.
    #[test]
    fn every_sub_page_is_linked_from_its_category() {
        for page in pages().iter().filter(|p| p.sub.is_some()) {
            // The profile pages are reached from the rows the profile list
            // draws, one per profile, rather than from a declared link - and
            // the macro editor likewise from the rows of the macro list.
            if matches!(page.sub, Some("profile" | "macro")) {
                continue;
            }
            let linked = pages().iter().any(|from| {
                from.sections().iter().any(|section| {
                    section
                        .items
                        .iter()
                        .any(|item| matches!(item.kind, Kind::Link(to) if to == page.route()))
                })
            });
            assert!(linked, "{:?} has no way in", page.route());
        }
    }

    /// The search leads to a row by its key, so two rows sharing one would
    /// send the highlight to the wrong one.
    #[test]
    fn no_two_rows_share_a_key() {
        let mut seen = Vec::new();
        for page in pages() {
            for section in page.sections() {
                for item in &section.items {
                    assert!(!seen.contains(&item.key), "{} is declared twice", item.key);
                    seen.push(item.key);
                }
            }
        }
    }

    /// The point of declaring each row once: whatever a page shows, its own
    /// title finds it, in either language.
    #[test]
    fn every_row_on_every_page_is_found_by_its_own_title_in_both_languages() {
        for page in pages() {
            for section in page.sections() {
                for item in &section.items {
                    for tr in [english as fn(_) -> _, portuguese] {
                        let hits = found(tr(item.title), tr);
                        assert!(
                            hits.contains(&item.key),
                            "{} is not found by {:?}",
                            item.key,
                            tr(item.title)
                        );
                    }
                }
            }
        }
    }

    /// A label left untranslated would read as English in the middle of the
    /// Portuguese window - and would only be found by its English name.
    #[test]
    fn every_category_page_section_and_row_title_is_translated() {
        let mut texts: Vec<&'static str> = Category::ALL.iter().map(|c| c.label()).collect();
        for page in pages() {
            texts.push(page.title);
            for section in page.sections() {
                texts.extend(section.heading);
                texts.extend(section.footer);
                for item in &section.items {
                    texts.push(item.title);
                    texts.extend(item.subtitle);
                    texts.extend(item.hint);
                }
            }
        }
        // Words that are the same in both languages.
        let same = [
            "Terminal",
            "Cursor",
            "Macros",
            "Shells",
            "Proxy",
            "Interface",
            "ANSI",
            "Macro",
            "Global",
            "String",
        ];
        for text in texts {
            if same.contains(&text) {
                continue;
            }
            assert_ne!(portuguese(text), text, "{text:?} has no translation");
        }
    }

    #[test]
    fn the_search_ignores_accents_and_case() {
        // "Núm. de Linhas de Rolagem", typed the way it is typed.
        assert!(found("NUM LINHAS DE ROLAGEM", portuguese).contains(&"scrollback_limit"));
        assert!(found("codificacao", portuguese).contains(&"profiles"));
    }

    #[test]
    fn a_hidden_keyword_finds_a_row_its_words_do_not_name() {
        assert!(found("bandeja", english).contains(&"close_to_tray"));
        assert!(found("senha", english).contains(&"proxy_password"));
        assert!(found("transparency", english).contains(&"th_ui_glass"));
    }

    #[test]
    fn the_portuguese_interface_still_finds_rows_by_their_english_names() {
        assert!(found("scrollback", portuguese).contains(&"scrollback_limit"));
    }

    #[test]
    fn a_row_on_a_sub_page_is_found_like_any_other() {
        assert!(found("proxy user", english).contains(&"proxy_user"));
        assert!(found("132", english).contains(&"cols_presets"));
    }

    #[test]
    fn a_word_that_names_nothing_finds_nothing() {
        assert!(found("xyzzy", english).is_empty());
    }

    #[test]
    fn the_feature_tips_are_rows_the_search_can_find() {
        assert!(found("ctrl+shift+t", english).contains(&"tip_tabs"));
    }

    #[test]
    fn a_search_result_names_the_sub_page_it_is_on_by_its_category_too() {
        let proxy = find_page(Route::sub(Category::About, "proxy")).unwrap();
        assert_eq!(proxy.breadcrumb(english), "About \u{203a} Proxy");
    }

    #[test]
    fn the_settings_window_first_opens_on_general_and_then_where_it_was_left() {
        let mut state = PanelState::default();
        assert_eq!(state.settings_route, Route::page(Category::General));
        state.settings_route = Route::sub(Category::About, "proxy");
        state.show_settings = true;
        // Closing is only the flag; the page is untouched by it.
        state.show_settings = false;
        assert_eq!(state.settings_route, Route::sub(Category::About, "proxy"));
    }

    /// The built-in themes, which is what a first start has.
    fn builtin_themes() -> Vec<Theme> {
        crate::config::theme::builtin_files()
            .iter()
            .map(|f| Theme::from_file(f).as_builtin())
            .collect()
    }

    /// A shared group and a personal one, a macro in each.
    fn some_macros() -> Vec<MacroGroup> {
        use crate::features::macros::{Macro, Origin, Param};
        vec![
            MacroGroup {
                name: "Shared".into(),
                origin: Origin::Organization,
                macros: vec![Macro {
                    origin: Origin::Organization,
                    name: "Org one".into(),
                    body: vec!["W 1".into()],
                    ..Macro::default()
                }],
            },
            MacroGroup {
                name: "Mine".into(),
                origin: Origin::Personal,
                macros: vec![Macro {
                    origin: Origin::Personal,
                    name: "Show global".into(),
                    params: vec![Param {
                        name: "g".into(),
                        ..Param::default()
                    }],
                    body: vec!["ZWRITE ^{{g}}".into()],
                    ..Macro::default()
                }],
            },
        ]
    }

    /// Draws the window headlessly - egui falls back to an embedded window
    /// with no backend to open a real one - with the built-in themes and
    /// `some_macros`, and returns what it asked for on its last frame.
    fn draw(state: &mut PanelState, settings: &mut Settings) -> Vec<UiRequest> {
        draw_with(state, settings, &mut builtin_themes(), &mut some_macros())
    }

    fn draw_with(
        state: &mut PanelState,
        settings: &mut Settings,
        themes: &mut Vec<Theme>,
        groups: &mut Vec<MacroGroup>,
    ) -> Vec<UiRequest> {
        let ctx = Context::default();
        let mut placement = crate::ui::detach::Placement::default();
        let buttons = crate::config::theme::WindowButtons::default();
        let mut requests = Vec::new();
        // A few frames: rows measure their controls on the first and settle
        // on the next, and a jump is carried out on the frame after a click.
        for _ in 0..3 {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                requests = settings_dialog(
                    ctx,
                    settings,
                    themes,
                    groups,
                    state,
                    &["IRIS".to_owned()],
                    &ServerList::default(),
                    &mut placement,
                    &buttons,
                );
            });
        }
        requests
    }

    fn quiet_state() -> PanelState {
        PanelState {
            show_settings: true,
            // Enumerating the system's fonts is slow, and nothing here is
            // about the font list.
            font_families: Some(Vec::new()),
            ..PanelState::default()
        }
    }

    /// Every page is drawn, with a profile so its page has something on it,
    /// and none of them panics or reports an edit nobody made.
    #[test]
    fn every_page_draws_without_changing_anything() {
        let mut settings = Settings {
            profiles: vec![Profile {
                name: "Local".into(),
                instance: "IRIS".into(),
                ..Profile::default()
            }],
            ..Settings::default()
        };
        for page in pages() {
            // The shells page probes the machine and writes what it finds
            // into the config folder, which a test must not touch.
            if page.category == Category::Sessions && page.sub == Some("shells") {
                continue;
            }
            let mut state = quiet_state();
            state.settings_route = page.route();
            let requests = draw(&mut state, &mut settings);
            assert!(
                requests.is_empty(),
                "{:?} reported {requests:?}",
                page.route()
            );
            assert!(state.show_settings);
        }
    }

    #[test]
    fn a_search_draws_its_results_and_keeps_the_page_it_was_on() {
        let mut settings = Settings::default();
        let mut state = quiet_state();
        state.settings_route = Route::page(Category::Windows);
        state.settings_search = "tray".into();
        assert!(draw(&mut state, &mut settings).is_empty());
        assert_eq!(state.settings_search, "tray");
        assert_eq!(state.settings_route, Route::page(Category::Windows));
    }

    #[test]
    fn the_page_of_a_profile_that_is_gone_goes_back_to_the_list() {
        let mut settings = Settings::default();
        let mut state = quiet_state();
        state.settings_route = Route {
            index: 3,
            ..Route::sub(Category::Sessions, "profile")
        };
        draw(&mut state, &mut settings);
        assert_eq!(state.settings_route, Route::page(Category::Sessions));
    }

    /// Every page the managers became, and every sub-page of theirs, is a
    /// route the window can be sent to.
    #[test]
    fn every_manager_page_and_sub_page_is_a_route_that_exists() {
        let routes = [
            Route::page(Category::Themes),
            Route::sub(Category::Themes, "theme"),
            Route::sub(Category::Themes, "theme_terminal"),
            Route::sub(Category::Themes, "theme_scrollbar"),
            Route::sub(Category::Themes, "theme_ansi"),
            Route::sub(Category::Themes, "theme_chrome"),
            Route::sub(Category::Themes, "theme_buttons"),
            Route::sub(Category::Themes, "theme_order"),
            Route::sub(Category::Themes, "theme_syntax"),
            Route::page(Category::ScreenSaver),
            Route::page(Category::Macros),
            Route::sub(Category::Macros, "macro"),
            Route::sub(Category::Macros, "org_macros"),
        ];
        for route in routes {
            assert!(find_page(route).is_some(), "{route:?} is not a page");
        }
    }

    /// The previews stay in view above the cards they preview.
    #[test]
    fn the_theme_and_screen_saver_pages_have_their_preview_on_top() {
        for route in [
            Route::page(Category::Themes),
            Route::sub(Category::Themes, "theme"),
            Route::sub(Category::Themes, "theme_syntax"),
            Route::page(Category::ScreenSaver),
        ] {
            assert!(find_page(route).unwrap().top.is_some(), "{route:?}");
        }
    }

    /// The macro manager's shortcut used to open its window; it now opens
    /// Settings on the macro list, wherever the window was left.
    #[test]
    fn the_macro_shortcut_opens_settings_on_the_macro_list() {
        assert_eq!(MACROS_PAGE, Route::page(Category::Macros));
        let mut state = PanelState {
            settings_route: Route::sub(Category::About, "proxy"),
            settings_search: "tray".into(),
            ..PanelState::default()
        };
        state.open_settings(MACROS_PAGE);
        assert!(state.show_settings);
        assert_eq!(state.settings_route, MACROS_PAGE);
        assert!(
            state.settings_search.is_empty(),
            "the results would hide the page"
        );
    }

    /// The way to the themes from Appearance is the Themes page itself, not
    /// a window of its own.
    #[test]
    fn the_appearance_page_leads_to_the_themes_page() {
        let appearance = find_page(Route::page(Category::Appearance)).unwrap();
        let link = appearance
            .sections()
            .iter()
            .flat_map(|s| &s.items)
            .find(|item| item.key == "theme_link")
            .expect("the link is there");
        assert!(matches!(link.kind, Kind::Link(to) if to == Route::page(Category::Themes)));
    }

    /// Opening the editor on a built-in and every one of its colour pages
    /// draws it - read-only - without writing anything, and without being
    /// sent back to the gallery for want of a theme.
    #[test]
    fn the_theme_editor_and_its_colour_pages_draw_a_built_in_without_writing_it() {
        let mut settings = Settings {
            theme: builtin_themes()[0].name.clone(),
            ..Settings::default()
        };
        for page in pages().iter().filter(|p| p.category == Category::Themes) {
            let mut state = quiet_state();
            state.settings_route = page.route();
            let requests = draw(&mut state, &mut settings);
            assert!(
                requests.is_empty(),
                "{:?} reported {requests:?}",
                page.route()
            );
            assert_eq!(state.settings_route, page.route(), "it was sent away");
        }
    }

    #[test]
    fn a_theme_page_with_no_theme_to_show_goes_back_to_the_gallery() {
        let mut settings = Settings::default();
        let mut state = quiet_state();
        state.settings_route = Route::sub(Category::Themes, "theme_chrome");
        draw_with(
            &mut state,
            &mut settings,
            &mut Vec::new(),
            &mut some_macros(),
        );
        assert_eq!(state.settings_route, Route::page(Category::Themes));
    }

    #[test]
    fn the_macro_editor_shows_the_selected_macro_and_stays_on_it() {
        let mut settings = Settings::default();
        let mut groups = some_macros();
        for at in [(0, 0), (1, 0)] {
            let mut state = quiet_state();
            state.macros.select(Some(at), &mut groups);
            state.settings_route = Route::sub(Category::Macros, "macro");
            let requests = draw_with(
                &mut state,
                &mut settings,
                &mut builtin_themes(),
                &mut groups,
            );
            assert!(requests.is_empty(), "{at:?} reported {requests:?}");
            assert_eq!(state.settings_route, Route::sub(Category::Macros, "macro"));
        }
    }

    #[test]
    fn the_macro_editor_with_nothing_selected_goes_back_to_the_list() {
        let mut settings = Settings::default();
        let mut state = quiet_state();
        state.settings_route = Route::sub(Category::Macros, "macro");
        draw(&mut state, &mut settings);
        assert_eq!(state.settings_route, MACROS_PAGE);
    }

    /// Esc, the mouse's back button and the header's link all lead one level
    /// up: from a colour page to the theme it belongs to, and from there to
    /// the gallery.
    #[test]
    fn the_way_back_from_a_colour_page_is_the_theme_editor_named_for_its_theme() {
        let group = find_page(Route::sub(Category::Themes, "theme_ansi")).unwrap();
        let editor = Route::sub(Category::Themes, "theme");
        assert_eq!(group.back(), Some(editor));
        assert_eq!(
            find_page(editor).unwrap().back(),
            Some(Route::page(Category::Themes))
        );
        assert!(find_page(editor).unwrap().heading.is_some());
        assert_eq!(
            find_page(Route::page(Category::Themes)).unwrap().back(),
            None
        );
    }

    /// A theme's colours are found by their names, and lead to the page they
    /// are on - not drawn as swatches out in the results, where nothing says
    /// which theme they would change.
    #[test]
    fn a_theme_colour_is_found_by_the_search_as_a_way_to_its_page() {
        assert!(found("selected tab", english).contains(&"th_tab"));
        assert!(found("cursor", english).contains(&"th_cursor"));
        assert!(found("cursor", english).contains(&"cursor_style"));
        assert!(found("global", portuguese).contains(&"th_syn_global"));
        let item = pages()
            .iter()
            .flat_map(|p| p.sections())
            .flat_map(|s| &s.items)
            .find(|item| item.key == "th_tab")
            .unwrap();
        assert!(item.contextual);
    }

    #[test]
    fn the_screen_saver_and_macro_options_are_found_by_the_search() {
        assert!(found("leading glyph", english).contains(&"saver_head"));
        assert!(found("minutos", portuguese).contains(&"saver_wait"));
        assert!(found("hide command", english).contains(&"macro_hide"));
        assert!(found("parametros", portuguese).contains(&"macro_params"));
        assert!(found("atalho", english).contains(&"macro_manager_shortcut"));
    }

    /// Results that are only ways to a page draw like any others, and edit
    /// nothing by being drawn.
    #[test]
    fn a_search_for_a_theme_colour_draws_without_editing_anything() {
        let mut settings = Settings::default();
        let mut state = quiet_state();
        state.settings_search = "background".into();
        let requests = draw(&mut state, &mut settings);
        assert!(requests.is_empty(), "{requests:?}");
    }
}
