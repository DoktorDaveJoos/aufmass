// No console window next to the app on Windows.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod plan;
mod scene;

use aufmass::model::{Kind, OpeningKind};
use aufmass::report::{self, de};
use aufmass::{collect_inputs, convert, default_out, telemetry, Config, Conversion, FileReport, GAEB_FILE};
use gpui::prelude::FluentBuilder;
use gpui::{
    actions, canvas, div, ClipboardItem, img, point, px, rgb, size, App, AppContext, Application, Bounds, Context, Corners, ExternalPaths,
    FontWeight, Image, ImageFormat, InteractiveElement, IntoElement, KeyBinding, Menu, MenuItem, MouseButton,
    MouseDownEvent, MouseMoveEvent, ParentElement, PathPromptOptions, Pixels, Point, Render, RenderImage,
    ScrollWheelEvent, SharedString, StatefulInteractiveElement, Styled, Task, TitlebarOptions,
    Window, WindowBounds, WindowOptions,
};
use scene::{Camera, Scene, Visibility};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

actions!(aufmass, [Quit]);

// The bauzertifikate.de design system (energieausweis-express/bauzertifikate globals.css):
// actions are black, the brand blue is an accent.
const BG: u32 = 0xf5f6f8;
const PANEL: u32 = 0xffffff;
const SUNKEN: u32 = 0xf0f1f4;
const BORDER: u32 = 0xe5e7ec;
const CONTROL_BORDER: u32 = 0x8a909c;
const TEXT: u32 = 0x0f1219;
const MUTED: u32 = 0x596070;
const PRIMARY_HOVER: u32 = 0x2a2f3a;
const ACCENT: u32 = 0x0000bd;
const ACCENT_SOFT: u32 = 0xebebf9;
const DANGER: u32 = 0xb42318;

/// The bauzertifikate.de mark, upright, about 2:3.
const MARK: &str = include_str!("../assets/mark.svg");

enum State {
    Empty,
    Working(String),
    Failed { message: String, files: Vec<String> },
    Done(Box<Done>),
}

struct Done {
    conversion: Conversion,
    out: PathBuf,
    /// Rasterized plan preview per storey.
    plans: Vec<(String, Option<Arc<RenderImage>>)>,
    scene: Arc<Scene>,
    /// Programs from the config, for "In PAPA öffnen" and "In cadwork öffnen".
    papa: Option<PathBuf>,
    cadwork: Option<PathBuf>,
}

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Model,
    Plan(usize),
}

struct Converter {
    state: State,
    tab: Tab,
    camera: Camera,
    visibility: Visibility,
    drag: Option<Point<Pixels>>,
    /// Storey the quantity list is limited to; `None` lists the whole building.
    summary_storey: Option<usize>,
    /// One-line message under the header, e.g. what to do in PAPA next.
    notice: Option<String>,
    /// Tab to show after the first conversion (`--tab` on the command line).
    initial_tab: Option<String>,
    _task: Option<Task<()>>,
}

impl Converter {
    fn start(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        if paths.is_empty() {
            return;
        }
        let names: Vec<String> =
            paths.iter().map(|p| p.file_name().unwrap_or_default().to_string_lossy().into_owned()).collect();
        self.state = State::Working(format!("Lese {} …", names.join(", ")));
        self.notice = None;
        let job = cx.background_executor().spawn(async move { run(paths) });
        self._task = Some(cx.spawn(async move |this, cx| {
            let state = job.await;
            this.update(cx, |this, cx| {
                if let State::Done(done) = &state {
                    this.visibility = Visibility { storeys: vec![true; done.plans.len()], roof: true };
                    this.camera = Camera::default();
                    this.summary_storey = None;
                    this.tab = match this.initial_tab.take() {
                        Some(t) => done.plans.iter().position(|(n, _)| n.eq_ignore_ascii_case(&t)).map_or(Tab::Model, Tab::Plan),
                        None => Tab::Model,
                    };
                }
                this.state = state;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// Starts PAPA and puts the GAEB file path on the clipboard; PAPA has no
    /// way to be handed a file to import, so the user pastes it in the GAEB import.
    fn open_in_papa(&mut self, cx: &mut Context<Self>) {
        let State::Done(done) = &self.state else { return };
        let file = done.out.join(GAEB_FILE);
        cx.write_to_clipboard(ClipboardItem::new_string(file.display().to_string()));
        let started = done.papa.as_ref().map(|exe| launch(exe, "PAPA"));
        self.notice = Some(match started {
            Some(Ok(_)) => "PAPA startet. Der Pfad zu mengen.x83 ist kopiert: in PAPA das Projekt öffnen, GAEB-Import wählen und den Pfad einfügen (Strg+V).".into(),
            Some(Err(e)) => {
                cx.reveal_path(&file);
                format!("{e} Der Ordner mit mengen.x83 ist geöffnet, der Pfad kopiert.")
            }
            None => {
                cx.reveal_path(&file);
                "Kein PAPA-Programm eingetragen (aufmass.toml, [papa] path). Der Ordner mit mengen.x83 ist geöffnet, der Pfad kopiert.".into()
            }
        });
        cx.notify();
    }

    /// Installs the import plugin into cadwork and starts cadwork; the plugin
    /// picks up this result on its own, so the user only runs it there.
    fn open_in_cadwork(&mut self, cx: &mut Context<Self>) {
        let State::Done(done) = &self.state else { return };
        let plugin = aufmass::cadwork::PLUGIN_NAME;
        let manual = if aufmass::cadwork::install_plugin().is_empty() {
            // No cadwork user profile found: leave the plugin next to the results.
            let dir = done.out.join("cadwork-plugin");
            match aufmass::cadwork::write_plugin(&dir) {
                Ok(()) => {
                    cx.reveal_path(&dir.join(plugin));
                    format!(" Kein cadwork-Benutzerprofil gefunden: den geöffneten Ordner „{plugin}“ einmalig nach …\\cadwork\\userprofil_XX\\3d\\API.x64 kopieren.")
                }
                Err(e) => format!(" Plugin konnte nicht gespeichert werden: {e:#}"),
            }
        } else {
            String::new()
        };
        let run = format!("In cadwork eine 3D-Datei öffnen und im Plugin-Menü „{plugin}“ wählen, die Auswertung wird übernommen.");
        self.notice = Some(match done.cadwork.as_ref().map(|exe| launch(exe, "cadwork")) {
            Some(Ok(())) => format!("cadwork startet. {run}{manual}"),
            Some(Err(e)) => format!("{e} {run}{manual}"),
            None => format!("Kein cadwork-Programm eingetragen (aufmass.toml, [cadwork] path). {run}{manual}"),
        });
        cx.notify();
    }

    fn choose(&mut self, directories: bool, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: !directories,
            directories,
            multiple: true,
            prompt: Some("Auswerten".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = picked.await {
                this.update(cx, |this, cx| this.start(paths, cx)).ok();
            }
        })
        .detach();
    }
}

/// Starts a program from its own folder, as older Windows programs expect.
fn launch(exe: &std::path::Path, name: &str) -> Result<(), String> {
    let mut cmd = std::process::Command::new(exe);
    if let Some(dir) = exe.parent() {
        cmd.current_dir(dir);
    }
    cmd.spawn().map(drop).map_err(|e| format!("{name} konnte nicht gestartet werden ({}): {e}.", exe.display()))
}

/// Runs on a background thread: read, classify, write outputs, prepare previews.
fn run(paths: Vec<PathBuf>) -> State {
    let fail = |e: anyhow::Error, files: Vec<String>| State::Failed { message: format!("{e:#}"), files };
    let cfg = match Config::load(None) {
        Ok(c) => c,
        Err(e) => return fail(e, vec![]),
    };
    let files = match collect_inputs(&paths) {
        Ok(f) => f,
        Err(e) => return fail(e, vec![]),
    };
    let out = default_out(&paths[0]);
    telemetry::set_folders(&paths, &out);
    let conversion = convert(&files, &cfg);
    let lines: Vec<String> = conversion.files.iter().map(FileReport::line).collect();
    if let Err(e) = conversion.check() {
        telemetry::conversion(&conversion.files, Some(&e));
        return fail(e, lines);
    }
    if let Err(e) = conversion.write(&out) {
        let e = e.context("Ergebnis konnte nicht gespeichert werden");
        telemetry::conversion(&conversion.files, Some(&e));
        return fail(e, lines);
    }
    telemetry::conversion(&conversion.files, None);
    let plans = conversion
        .building
        .storeys
        .iter()
        .map(|s| (s.name.clone(), plan::render(&conversion.preview_svg(s), 2.0).ok()))
        .collect();
    let scene = Arc::new(Scene::new(&conversion.building, conversion.colors()));
    State::Done(Box::new(Done { conversion, out, plans, scene, papa: cfg.papa.path.clone(), cadwork: cfg.cadwork.path.clone() }))
}

impl Render for Converter {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match &self.state {
            State::Empty => self.empty(cx).into_any_element(),
            State::Working(msg) => centered(div().text_color(rgb(MUTED)).child(msg.clone())).into_any_element(),
            State::Failed { message, files } => self.failed(message, files, cx).into_any_element(),
            State::Done(done) => self.done(done, cx).into_any_element(),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(BG))
            .text_color(rgb(TEXT))
            .text_sm()
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| this.start(paths.paths().to_vec(), cx)))
            .drag_over::<ExternalPaths>(|style, _, _, _| style.bg(rgb(ACCENT_SOFT)))
            .child(self.header(cx))
            .when_some(self.notice.clone(), |d, notice| {
                d.child(
                    div()
                        .id("notice")
                        .flex()
                        .gap_3()
                        .px_4()
                        .py_2()
                        .bg(rgb(ACCENT_SOFT))
                        .border_b_1()
                        .border_color(rgb(BORDER))
                        .cursor_pointer()
                        .child(div().flex_1().child(notice))
                        .child(div().text_color(rgb(MUTED)).child("✕"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.notice = None;
                            cx.notify();
                        })),
                )
            })
            .child(div().flex_1().min_h_0().p_4().child(body))
    }
}

impl Converter {
    fn header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let done = match &self.state {
            State::Done(d) => Some(d.out.clone()),
            _ => None,
        };
        div()
            .flex()
            .items_center()
            .justify_between()
            .px_4()
            .py_3()
            .bg(rgb(PANEL))
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(logo())
            .child(
                div()
                    .flex()
                    .gap_2()
                    .when_some(done, |row, out| {
                        let xlsx = out.join("mengen.xlsx");
                        row.child(button("open-papa", "In PAPA öffnen", true).on_click(cx.listener(|this, _, _, cx| this.open_in_papa(cx))))
                            .child(button("open-cadwork", "In cadwork öffnen", true).on_click(cx.listener(|this, _, _, cx| this.open_in_cadwork(cx))))
                            .child(button("open-xlsx", "Excel öffnen", false).on_click(cx.listener(move |_, _, _, cx| cx.open_with_system(&xlsx))))
                            .child(button("reveal", "Ordner zeigen", false).on_click(cx.listener(move |_, _, _, cx| cx.reveal_path(&out))))
                    })
                    .child(button("pick-dir", "Ordner wählen …", false).on_click(cx.listener(|this, _, _, cx| this.choose(true, cx))))
                    .child(button("pick-files", "Dateien wählen …", false).on_click(cx.listener(|this, _, _, cx| this.choose(false, cx)))),
            )
    }

    fn empty(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_3()
            .rounded_lg()
            .border_2()
            .border_dashed()
            .border_color(rgb(CONTROL_BORDER))
            .child(div().text_xl().font_weight(FontWeight::SEMIBOLD).child("Pläne hierher ziehen"))
            .child(div().text_color(rgb(MUTED)).child("Einen Ordner oder einzelne DWG-/DXF-Dateien vom Architekten"))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .mt_2()
                    .child(button("empty-dir", "Ordner wählen …", true).on_click(cx.listener(|this, _, _, cx| this.choose(true, cx))))
                    .child(button("empty-files", "Dateien wählen …", false).on_click(cx.listener(|this, _, _, cx| this.choose(false, cx)))),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .mt_4()
                    .child("Die Ergebnisse werden im Ordner „aufmass“ neben den Plänen gespeichert."),
            )
    }

    fn failed(&self, message: &str, files: &[String], _cx: &mut Context<Self>) -> impl IntoElement {
        centered(
            div()
                .max_w(px(720.))
                .flex()
                .flex_col()
                .gap_3()
                .p_5()
                .rounded_lg()
                .bg(rgb(PANEL))
                .border_1()
                .border_color(rgb(BORDER))
                .child(div().text_color(rgb(DANGER)).font_weight(FontWeight::SEMIBOLD).child("Keine Auswertung möglich"))
                .child(div().child(message.to_string()))
                .when(!files.is_empty(), |d| {
                    d.child(
                        div()
                            .flex()
                            .flex_col()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .font_family(if cfg!(windows) { "Consolas" } else { "Menlo" })
                            .children(files.iter().map(|l| div().child(l.clone()))),
                    )
                })
                .child(div().text_xs().text_color(rgb(MUTED)).child("Andere Pläne einfach hierher ziehen.")),
        )
    }

    fn done(&self, done: &Done, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .gap_4()
            .child(self.summary(done, cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .rounded_lg()
                    .bg(rgb(PANEL))
                    .border_1()
                    .border_color(rgb(BORDER))
                    .overflow_hidden()
                    .child(self.tabs(done, cx))
                    .child(match self.tab {
                        Tab::Model => self.model(done, cx).into_any_element(),
                        Tab::Plan(i) => plan_view(done.plans.get(i).and_then(|(_, img)| img.clone())).into_any_element(),
                    }),
            )
    }

    fn tabs(&self, done: &Done, cx: &mut Context<Self>) -> impl IntoElement {
        let mut tabs = vec![(Tab::Model, SharedString::from("3D-Modell"))];
        tabs.extend(done.plans.iter().enumerate().map(|(i, (n, _))| (Tab::Plan(i), SharedString::from(format!("Grundriss {n}")))));
        div().flex().gap_1().px_2().pt_2().border_b_1().border_color(rgb(BORDER)).children(tabs.into_iter().enumerate().map(
            |(i, (tab, label))| {
                let active = self.tab == tab;
                div()
                    .id(("tab", i))
                    .px_3()
                    .py_1p5()
                    .cursor_pointer()
                    .border_b_2()
                    .border_color(rgb(if active { ACCENT } else { PANEL }))
                    .text_color(rgb(if active { TEXT } else { MUTED }))
                    .when(active, |d| d.font_weight(FontWeight::SEMIBOLD))
                    .hover(|s| s.text_color(rgb(TEXT)))
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.tab = tab;
                        cx.notify();
                    }))
            },
        ))
    }

    fn model(&self, done: &Done, cx: &mut Context<Self>) -> impl IntoElement {
        let scene = done.scene.clone();
        let camera = self.camera;
        let visibility = self.visibility.clone();
        let toggles = done.plans.iter().enumerate().map(|(i, (name, _))| {
            let on = self.visibility.storeys.get(i).copied().unwrap_or(true);
            toggle(("storey", i), name.clone(), on).on_click(cx.listener(move |this, _, _, cx| {
                if let Some(v) = this.visibility.storeys.get_mut(i) {
                    *v = !*v;
                }
                cx.notify();
            }))
        });
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .children(toggles)
                    .child(toggle("roof", "Dach", self.visibility.roof).on_click(cx.listener(|this, _, _, cx| {
                        this.visibility.roof = !this.visibility.roof;
                        cx.notify();
                    })))
                    .child(button("reset-view", "Ansicht zurücksetzen", false).on_click(cx.listener(|this, _, _, cx| {
                        this.camera = Camera::default();
                        cx.notify();
                    })))
                    .child(div().ml_auto().text_xs().text_color(rgb(MUTED)).child("Ziehen: drehen · Scrollen: zoomen")),
            )
            .child(
                div()
                    .id("model-canvas")
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .overflow_hidden()
                    .bg(rgb(0xfafbfc))
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, e: &MouseDownEvent, _, _| this.drag = Some(e.position)))
                    .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| {
                        let (Some(from), Some(MouseButton::Left)) = (this.drag, e.pressed_button) else {
                            this.drag = None;
                            return;
                        };
                        let d = e.position - from;
                        this.camera.yaw -= f32::from(d.x) * 0.008;
                        this.camera.pitch = (this.camera.pitch + f32::from(d.y) * 0.008).clamp(-0.2, 1.5);
                        this.drag = Some(e.position);
                        cx.notify();
                    }))
                    .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.drag = None))
                    .on_scroll_wheel(cx.listener(|this, e: &ScrollWheelEvent, _, cx| {
                        let dy = f32::from(e.delta.pixel_delta(px(20.)).y);
                        this.camera.zoom = (this.camera.zoom * (1.0 + dy * 0.002)).clamp(0.3, 8.0);
                        cx.notify();
                    }))
                    .child(canvas(|_, _, _| {}, move |bounds, _, window, _| scene.paint(camera, &visibility, bounds, window)).size_full())
                    .child(legend(done)),
            )
    }
}

impl Converter {
    fn summary(&self, done: &Done, cx: &mut Context<Self>) -> impl IntoElement {
        let b = &done.conversion.building;
        let storeys: Vec<_> =
            b.storeys.iter().enumerate().filter(|(i, _)| self.summary_storey.is_none_or(|s| s == *i)).map(|(_, s)| s).collect();
        let rows = report::totals(storeys.iter().copied());
        let openings: Vec<_> = storeys.iter().flat_map(|s| s.elements.iter().flat_map(|e| &e.openings)).collect();
        let count = |k: OpeningKind| openings.iter().filter(|o| o.kind == k).count();
        let sections = Kind::ALL.into_iter().filter(|&kind| rows.keys().any(|(k, _)| *k == kind)).map(|kind| {
            let lines = rows.iter().filter(|((k, _), _)| *k == kind).map(|((_, name), t)| quantity_row(name.clone(), t, false));
            div()
                .flex()
                .flex_col()
                .child(div().mt_3().mb_1().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(rgb(MUTED)).child(kind.plural()))
                .children(lines)
                .child(quantity_row(format!("Summe {}", kind.plural()), &report::subtotal(&rows, kind), true))
        });
        let total = report::grand_total(&rows);
        let filter = (b.storeys.len() > 1).then(|| {
            let chips = std::iter::once((None, "Alle".to_string())).chain(b.storeys.iter().enumerate().map(|(i, s)| (Some(i), s.name.clone())));
            div().flex().flex_wrap().gap_1().mt_2().children(chips.enumerate().map(|(k, (storey, label))| {
                toggle(("summary-storey", k), label, self.summary_storey == storey).on_click(cx.listener(move |this, _, _, cx| {
                    this.summary_storey = storey;
                    cx.notify();
                }))
            }))
        });
        div()
            .id("summary")
            .w(px(440.))
            .flex_none()
            .flex()
            .flex_col()
            .p_4()
            .rounded_lg()
            .bg(rgb(PANEL))
            .border_1()
            .border_color(rgb(BORDER))
            .overflow_y_scroll()
            .child(div().font_weight(FontWeight::SEMIBOLD).child("Mengen"))
            .children(filter)
            .child(
                div()
                    .flex()
                    .gap_2()
                    .mt_2()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .child(div().flex_1().child("Bezeichnung"))
                    .child(div().w(px(36.)).text_right().child("Anz."))
                    .child(div().w(px(88.)).text_right().child("Fläche netto"))
                    .child(div().w(px(70.)).text_right().child("Volumen")),
            )
            .children(sections)
            .child(
                div()
                    .flex()
                    .gap_2()
                    .mt_3()
                    .pt_2()
                    .border_t_2()
                    .border_color(rgb(TEXT))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(div().flex_1().child("Gesamt"))
                    .child(div().w(px(36.)).text_right().child(total.count.to_string()))
                    .child(div().w(px(88.)))
                    .child(div().w(px(70.)).text_right().child(format!("{} m³", de(total.volume, 2)))),
            )
            .child(div().mt_3().text_xs().text_color(rgb(MUTED)).child(format!(
                "Öffnungen in allen Wandschichten: {} Türen, {} Fenster, {} sonstige",
                count(OpeningKind::Door),
                count(OpeningKind::Window),
                count(OpeningKind::Other)
            )))
            .child(div().mt_4().font_weight(FontWeight::SEMIBOLD).child("Dateien"))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .mt_1()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .children(done.conversion.files.iter().map(|f| {
                        let used = f.elements > 0;
                        div()
                            .flex()
                            .gap_2()
                            .child(div().flex_1().text_color(rgb(if used { TEXT } else { MUTED })).child(f.file.clone()))
                            .child(div().child(match (&f.error, f.elements) {
                                (Some(_), _) => "nicht lesbar".to_string(),
                                (None, 0) if f.solids > 0 => "nur Doppelte, Bestand, Möbel".to_string(),
                                (None, 0) if f.unreadable > 0 => "3D-Daten nicht lesbar".to_string(),
                                (None, 0) => "kein 3D-Modell".to_string(),
                                (None, n) => format!("{n} Bauteile"),
                            }))
                    })),
            )
            .child(div().mt_4().text_xs().text_color(rgb(MUTED)).child(format!("Gespeichert in {}", done.out.display())))
    }
}

/// One line of the quantity list; `total` lines are bold with a rule above.
fn quantity_row(name: String, t: &report::Sum, total: bool) -> impl IntoElement {
    div()
        .flex()
        .gap_2()
        .py_1()
        .when(!total, |d| d.border_b_1().border_color(rgb(SUNKEN)))
        .when(total, |d| d.font_weight(FontWeight::SEMIBOLD))
        .child(div().flex_1().min_w_0().child(name))
        .child(div().w(px(36.)).text_right().when(!total, |d| d.text_color(rgb(MUTED))).child(t.count.to_string()))
        .child(div().w(px(88.)).text_right().child(format!("{} m²", de(t.net, 2))))
        .child(div().w(px(70.)).text_right().when(!total, |d| d.text_color(rgb(MUTED))).child(format!("{} m³", de(t.volume, 2))))
}

/// Paints the plan scaled to fit. An `img` element would size itself from the
/// picture's aspect ratio inside this flex layout and overflow the panel.
fn plan_view(image: Option<Arc<RenderImage>>) -> impl IntoElement {
    let Some(image) = image else {
        return centered(div().text_color(rgb(MUTED)).child("Vorschau nicht verfügbar")).into_any_element();
    };
    div()
        .flex_1()
        .min_h_0()
        .child(
            canvas(|_, _, _| {}, move |bounds, _, window, _| {
                let pixels = image.size(0);
                let (iw, ih) = (pixels.width.0 as f32, pixels.height.0 as f32);
                let pad = 12.0;
                let (bw, bh) = (f32::from(bounds.size.width) - 2.0 * pad, f32::from(bounds.size.height) - 2.0 * pad);
                let k = (bw / iw).min(bh / ih);
                let (w, h) = (iw * k, ih * k);
                let origin = bounds.origin + point(px(pad + (bw - w) / 2.0), px(pad + (bh - h) / 2.0));
                window.paint_image(Bounds::new(origin, size(px(w), px(h))), Corners::default(), image.clone(), 0, false).ok();
            })
            .size_full(),
        )
        .into_any_element()
}

fn legend(done: &Done) -> impl IntoElement {
    let walls = done.conversion.colors().iter().map(|(name, color)| (name.clone(), scene::hex(color)));
    let entries: Vec<(String, u32)> =
        walls.chain([("Decken".to_string(), scene::SLAB_COLOR), ("Dach".to_string(), scene::ROOF_COLOR)]).collect();
    div()
        .absolute()
        .left_3()
        .bottom_3()
        .flex()
        .flex_col()
        .gap_1()
        .p_2()
        .rounded_md()
        .bg(rgb(PANEL))
        .border_1()
        .border_color(rgb(BORDER))
        .text_xs()
        .children(entries.into_iter().map(|(name, color)| {
            div().flex().items_center().gap_2().child(div().size(px(10.)).rounded_sm().bg(rgb(color))).child(name)
        }))
}

/// "Aufmaß / by bauzertifikate.de" next to the mark, as the bauzertifikate.de shops show their names.
fn logo() -> impl IntoElement {
    static MARK_PNG: OnceLock<Option<Arc<Image>>> = OnceLock::new();
    let mark = MARK_PNG.get_or_init(|| {
        // Twice the 26pt it is shown at, sharp on Retina.
        let png = plan::rasterize(MARK, 52.0 / 228.36).ok()?;
        Some(Arc::new(Image::from_bytes(ImageFormat::Png, png)))
    });
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .when_some(mark.clone(), |d, mark| d.child(img(mark).h(px(26.)).w(px(26. * 152.24 / 228.36))))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(div().text_size(px(17.)).line_height(px(18.)).font_weight(FontWeight::SEMIBOLD).child("Aufmaß"))
                .child(div().text_size(px(11.)).line_height(px(12.)).text_color(rgb(MUTED)).child("by bauzertifikate.de")),
        )
}

fn button(id: &'static str, label: &'static str, primary: bool) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .px_3()
        .py_1p5()
        .rounded_md()
        .border_1()
        .cursor_pointer()
        .when(primary, |d| d.bg(rgb(TEXT)).border_color(rgb(TEXT)).text_color(rgb(0xffffff)).hover(|s| s.bg(rgb(PRIMARY_HOVER))))
        .when(!primary, |d| d.bg(rgb(PANEL)).border_color(rgb(BORDER)).hover(|s| s.bg(rgb(SUNKEN))))
        .child(label)
}

fn toggle(id: impl Into<gpui::ElementId>, label: impl Into<SharedString>, on: bool) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .px_2p5()
        .py_1()
        .rounded_md()
        .border_1()
        .cursor_pointer()
        .text_xs()
        .when(on, |d| d.bg(rgb(ACCENT_SOFT)).border_color(rgb(ACCENT)).text_color(rgb(ACCENT)))
        .when(!on, |d| d.bg(rgb(PANEL)).border_color(rgb(BORDER)).text_color(rgb(MUTED)))
        .child(label.into())
}

fn centered(child: impl IntoElement) -> gpui::Div {
    div().size_full().flex().items_center().justify_center().child(child)
}

fn main() {
    // Crashes and failed conversions; the window has no console to print them to.
    let _sentry = telemetry::init("app");
    let mut paths = Vec::new();
    let mut initial_tab = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--tab" {
            initial_tab = args.next();
        } else {
            paths.push(PathBuf::from(a));
        }
    }

    Application::new().run(move |cx: &mut App| {
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.bind_keys([KeyBinding::new(if cfg!(target_os = "macos") { "cmd-q" } else { "ctrl-q" }, Quit, None)]);
        cx.set_menus(vec![Menu { name: "Aufmaß".into(), items: vec![MenuItem::action("Aufmaß beenden", Quit)] }]);
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        let bounds = Bounds::centered(None, size(px(1440.), px(920.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions { title: Some("Aufmaß".into()), ..Default::default() }),
                window_min_size: Some(size(px(960.), px(600.))),
                ..Default::default()
            },
            move |_, cx| {
                cx.new(move |cx| {
                    let mut view = Converter {
                        state: State::Empty,
                        tab: Tab::Model,
                        camera: Camera::default(),
                        visibility: Visibility { storeys: vec![], roof: true },
                        drag: None,
                        summary_storey: None,
                        notice: None,
                        initial_tab,
                        _task: None,
                    };
                    view.start(paths, cx);
                    view
                })
            },
        )
        .expect("opening the window failed");
        cx.activate(true);
    });
}
