//! The start window, as in Thermkern: the projects opened lately on the left,
//! newest first and grouped by day, and on the right the ways to begin: a new
//! project or a project file from this computer. Laid out as Visual Studio's
//! start window, so it reads the same to Windows and Mac users. Project files
//! dropped onto it open; plans dropped onto it become a new project.

use crate::recent::{Entry, Recent};
use crate::{places, ACCENT, ACCENT_SOFT, BG, BORDER, CONTROL_BORDER, DANGER, DANGER_SOFT, FAINT, MUTED, NAME, PANEL, SUNKEN, TEXT};
use gpui::prelude::FluentBuilder;
use gpui::{
    actions, div, img, px, rgb, svg, App, AssetSource, Context, ExternalPaths, FocusHandle, FontWeight, Image, ImageFormat, InteractiveElement,
    IntoElement, ParentElement, Render, ScrollHandle, SharedString, StatefulInteractiveElement, Styled, Svg, Window,
};
use jiff::{Timestamp, Zoned};
use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::Arc;

actions!(start, [SelectPrevious, SelectNext, Confirm, Forget]);

pub struct Start {
    focus: FocusHandle,
    mark: Option<Arc<Image>>,
    /// The recent projects whose file has gone, looked up when the list changes.
    missing: Vec<PathBuf>,
    /// The entry chosen with the arrow keys.
    selected: Option<usize>,
    scroll: ScrollHandle,
    /// What went wrong last, until dismissed or the next try.
    pub error: Option<String>,
}

impl Start {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus);
        cx.observe_global::<Recent>(|this, cx| {
            this.look_up(cx);
            cx.notify();
        })
        .detach();
        cx.observe_global::<crate::Updates>(|_, cx| cx.notify()).detach();
        let mut start = Start { focus, mark: mark(), missing: Vec::new(), selected: None, scroll: ScrollHandle::new(), error: None };
        start.look_up(cx);
        start
    }

    fn look_up(&mut self, cx: &App) {
        let entries = &cx.global::<Recent>().entries;
        self.missing = entries.iter().filter(|e| !e.path.is_file()).map(|e| e.path.clone()).collect();
        self.selected = self.selected.filter(|&i| i < entries.len());
    }

    fn open(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.error = crate::open_project(&path, cx).err().map(|e| crate::failed(&path, &e));
        cx.notify();
    }

    /// Project files open; anything else is taken as plans for a new project.
    fn dropped(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let (projects, plans): (Vec<PathBuf>, Vec<PathBuf>) = paths.into_iter().partition(|p| places::is_project(p));
        let errors: Vec<String> =
            projects.iter().filter_map(|path| crate::open_project(path, cx).err().map(|e| crate::failed(path, &e))).collect();
        self.error = (!errors.is_empty()).then(|| errors.join("\n"));
        if !plans.is_empty() {
            crate::new_project(plans, cx);
        }
        cx.notify();
    }

    fn select(&mut self, step: isize, cx: &mut Context<Self>) {
        let n = cx.global::<Recent>().entries.len();
        if n == 0 {
            return;
        }
        let i = match self.selected {
            None if step > 0 => 0,
            None => n - 1,
            Some(i) => i.saturating_add_signed(step).min(n - 1),
        };
        self.selected = Some(i);
        // The list's children are the day headings and the rows; find this row's place among them.
        let entries = &cx.global::<Recent>().entries;
        let now = Zoned::now();
        let headings = (0..=i).filter(|&k| k == 0 || day(&entries[k], &now).0 != day(&entries[k - 1], &now).0).count();
        self.scroll.scroll_to_item(i + headings);
        cx.notify();
    }

    fn confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        let path = self.selected.and_then(|i| cx.global::<Recent>().entries.get(i)).map(|e| e.path.clone());
        if let Some(path) = path {
            self.open(path, cx);
        }
    }

    fn forget(&mut self, _: &Forget, _: &mut Window, cx: &mut Context<Self>) {
        let path = self.selected.and_then(|i| cx.global::<Recent>().entries.get(i)).map(|e| e.path.clone());
        if let Some(path) = path {
            crate::forget(&path, cx);
        }
    }

    fn recent(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let entries = cx.global::<Recent>().entries.clone();
        let now = Zoned::now();
        let mut list = div().id("recent").flex_1().overflow_y_scroll().track_scroll(&self.scroll).px_3().pb_4();
        let mut last = "";
        for (i, entry) in entries.iter().enumerate() {
            let (group, when) = day(entry, &now);
            if group != last {
                list = list.child(div().px_3().pt_4().pb_1().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(rgb(MUTED)).child(group));
                last = group;
            }
            let missing = self.missing.iter().any(|p| places::same(p, &entry.path));
            list = list.child(self.row(i, entry, when, missing, cx));
        }
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .bg(rgb(PANEL))
            .child(
                div()
                    .px_6()
                    .pt_5()
                    .pb_1()
                    .flex()
                    .items_baseline()
                    .gap_2()
                    .child(div().text_base().font_weight(FontWeight::SEMIBOLD).child("Zuletzt geöffnet"))
                    .when(!entries.is_empty(), |d| d.child(div().text_xs().text_color(rgb(MUTED)).child(format!("{}", entries.len())))),
            )
            .when_some(self.error.clone(), |d, error| d.child(self.banner(error, cx)))
            .when(entries.is_empty(), |d| d.child(empty()))
            .when(!entries.is_empty(), |d| d.child(list))
    }

    fn row(&self, i: usize, entry: &Entry, when: String, missing: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let open = entry.path.clone();
        let gone = entry.path.clone();
        let selected = self.selected == Some(i);
        let folder = places::folder(&entry.path);
        div()
            .id(("project", i))
            .group("row")
            .flex()
            .items_center()
            .gap_3()
            .px_3()
            .py_2()
            .rounded_md()
            .cursor_pointer()
            .when(selected, |d| d.bg(rgb(ACCENT_SOFT)))
            .when(!selected, |d| d.hover(|s| s.bg(rgb(SUNKEN))))
            .child(icon("house").size(px(18.)).text_color(rgb(if missing { FAINT } else { MUTED })))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(div().truncate().font_weight(FontWeight::MEDIUM).when(missing, |d| d.text_color(rgb(MUTED))).child(entry.name.clone()))
                    .child(div().truncate().text_xs().text_color(rgb(MUTED)).child(if missing { format!("Nicht gefunden · {folder}") } else { folder })),
            )
            .child(div().flex_none().text_xs().text_color(rgb(MUTED)).child(when))
            .child(
                div()
                    .id(("forget", i))
                    .flex_none()
                    .p_1()
                    .rounded_sm()
                    .invisible()
                    .group_hover("row", |s| s.visible())
                    .when(selected, |d| d.visible())
                    .hover(|s| s.bg(rgb(BORDER)))
                    .child(icon("close").size(px(14.)).text_color(rgb(MUTED)))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.stop_propagation();
                        crate::forget(&gone, cx);
                    })),
            )
            .on_click(cx.listener(move |this, _, _, cx| this.open(open.clone(), cx)))
    }

    fn banner(&self, error: String, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .mx_6()
            .mt_3()
            .px_3()
            .py_2()
            .flex()
            .items_start()
            .gap_2()
            .rounded_md()
            .border_1()
            .border_color(rgb(DANGER))
            .bg(rgb(DANGER_SOFT))
            .child(div().flex_1().min_w_0().child(error))
            .child(
                div()
                    .id("dismiss")
                    .p_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(BORDER)))
                    .child(icon("close").size(px(14.)).text_color(rgb(MUTED)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.error = None;
                        cx.notify();
                    })),
            )
    }

    fn begin(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("begin")
            .overflow_y_scroll()
            .w(px(340.))
            .flex_none()
            .flex()
            .flex_col()
            .gap_3()
            .p_6()
            .border_l_1()
            .border_color(rgb(BORDER))
            .child(brand(self.mark.clone()))
            .child(heading("Loslegen"))
            .child(
                tile("new", "plus", "Neues Projekt", "Leere Projektdatei anlegen", shortcut("N"))
                    .on_click(cx.listener(|_, _, _, cx| crate::new_project(Vec::new(), cx))),
            )
            .child(tile("open", "folder", "Projekt öffnen …", "Datei auf diesem Computer", shortcut("O")).on_click(cx.listener(|_, _, _, cx| crate::open_dialog(cx))))
            .child(
                div()
                    .pt_1()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .child("Ein Ordner mit Plänen, hierher gezogen, wird gleich ein neues Projekt."),
            )
            .child(div().flex_1())
            .child(div().text_xs().text_color(rgb(MUTED)).child(format!("Version {}", aufmass::update::VERSION)))
    }
}

impl Render for Start {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("start")
            .key_context("Start")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.select(1, cx)))
            .on_action(cx.listener(|this, _: &SelectPrevious, _, cx| this.select(-1, cx)))
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::forget))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| this.dropped(paths.paths().to_vec(), cx)))
            .drag_over::<ExternalPaths>(|style, _, _, _| style.bg(rgb(ACCENT_SOFT)))
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(BG))
            .text_color(rgb(TEXT))
            .text_sm()
            .when_some(crate::update_bar(cx), |d, bar| d.child(bar))
            .child(div().flex_1().min_h_0().flex().child(self.recent(cx)).child(self.begin(cx)))
    }
}

/// The bauzertifikate.de mark, twice the 32pt it is shown at, sharp on Retina.
fn mark() -> Option<Arc<Image>> {
    let png = crate::plan::rasterize(crate::MARK, 64.0 / 228.36).ok()?;
    Some(Arc::new(Image::from_bytes(ImageFormat::Png, png)))
}

/// The mark and "Aufmaß / by bauzertifikate.de", as Thermkern and the shops show their names.
fn brand(mark: Option<Arc<Image>>) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(10.))
        .pb_3()
        .when_some(mark, |d, mark| d.child(img(mark).h(px(32.)).w(px(32. * 152.24 / 228.36))))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(div().text_size(px(20.)).line_height(px(22.)).font_weight(FontWeight::SEMIBOLD).child(NAME))
                .child(div().text_size(px(11.)).line_height(px(12.)).text_color(rgb(MUTED)).child("by bauzertifikate.de")),
        )
}

fn heading(text: &'static str) -> impl IntoElement {
    div().pt_2().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(rgb(MUTED)).child(text)
}

fn tile(id: &'static str, symbol: &'static str, title: &'static str, detail: &'static str, keys: String) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap_3()
        .p_3()
        .rounded_lg()
        .border_1()
        .border_color(rgb(BORDER))
        .bg(rgb(PANEL))
        .cursor_pointer()
        .hover(|s| s.border_color(rgb(CONTROL_BORDER)))
        .child(
            div()
                .size(px(36.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded_md()
                .bg(rgb(ACCENT_SOFT))
                .child(icon(symbol).size(px(18.)).text_color(rgb(ACCENT))),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
                .child(div().text_xs().text_color(rgb(MUTED)).child(detail)),
        )
        .child(div().flex_none().text_xs().text_color(rgb(MUTED)).child(keys))
}

fn empty() -> impl IntoElement {
    div()
        .flex_1()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_2()
        .px_8()
        .child(icon("house").size(px(28.)).text_color(rgb(FAINT)))
        .child(div().pt_1().text_base().font_weight(FontWeight::SEMIBOLD).child("Noch keine Projekte"))
        .child(div().max_w(px(360.)).text_center().text_color(rgb(MUTED)).child(
            "Rechts ein neues Projekt anlegen oder eine Projektdatei öffnen, oder einen Ordner mit Plänen hierher ziehen. Projekte bleiben als Datei auf diesem Computer.",
        ))
}

/// A shortcut as the platform writes it: "⌘N" on macOS, "Strg+N" on Windows' German keyboards.
fn shortcut(key: &str) -> String {
    if cfg!(target_os = "macos") { format!("⌘{key}") } else { format!("Strg+{key}") }
}

const WEEKDAYS: [&str; 7] = ["Mo.", "Di.", "Mi.", "Do.", "Fr.", "Sa.", "So."];
const MONTHS: [&str; 12] = ["Jan.", "Feb.", "März", "Apr.", "Mai", "Juni", "Juli", "Aug.", "Sept.", "Okt.", "Nov.", "Dez."];

/// The day group an entry falls into and when it was opened, in the local time zone: "Heute 14:32", "Gestern",
/// a weekday within a week, else a date.
fn day(entry: &Entry, now: &Zoned) -> (&'static str, String) {
    let Ok(at) = Timestamp::from_second(entry.opened).map(|t| t.to_zoned(now.time_zone().clone())) else {
        return ("Älter", String::new());
    };
    let days = at.date().until(now.date()).map(|s| s.get_days()).unwrap_or(i32::MAX);
    let time = format!("{:02}:{:02}", at.hour(), at.minute());
    match days {
        ..=0 => ("Heute", time),
        1 => ("Gestern", time),
        2..=6 => ("Letzte 7 Tage", format!("{} {time}", WEEKDAYS[at.weekday().to_monday_zero_offset() as usize])),
        _ => ("Älter", format!("{}. {} {}", at.day(), MONTHS[at.month() as usize - 1], at.year())),
    }
}

/// The line icons (assets/icons, 24 × 24, from Thermkern).
const ICONS: &[(&str, &[u8])] = &[
    ("icons/close.svg", include_bytes!("../assets/icons/close.svg")),
    ("icons/folder.svg", include_bytes!("../assets/icons/folder.svg")),
    ("icons/house.svg", include_bytes!("../assets/icons/house.svg")),
    ("icons/plus.svg", include_bytes!("../assets/icons/plus.svg")),
];

/// The icons, compiled into the app.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        Ok(ICONS.iter().find(|(p, _)| *p == path).map(|(_, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        Ok(ICONS.iter().filter(|(p, _)| p.starts_with(path)).map(|(p, _)| SharedString::from(*p)).collect())
    }
}

/// An icon by name, in the text colour unless given one. GPUI takes only its shape.
fn icon(name: &'static str) -> Svg {
    svg().path(format!("icons/{name}.svg")).flex_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_by_day() {
        let now = Zoned::now();
        let at = |days: i64| Entry { path: PathBuf::new(), name: String::new(), opened: now.timestamp().as_second() - days * 86_400 };
        assert_eq!(day(&at(0), &now).0, "Heute");
        assert_eq!(day(&at(1), &now).0, "Gestern");
        assert_eq!(day(&at(4), &now).0, "Letzte 7 Tage");
        assert_eq!(day(&at(30), &now).0, "Älter");
    }
}
