// No console window next to the app on Windows.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod plan;
mod scene;

use aufmass::model::{Kind, OpeningKind};
use aufmass::report::{self, de};
use aufmass::programs::{self, Program};
use aufmass::{collect_inputs, convert_with_progress, default_out, telemetry, update, Config, Conversion, FileReport, GAEB_FILE};
use gpui::prelude::FluentBuilder;
use gpui::{
    actions, canvas, div, relative, Animation, AnimationExt, ClipboardItem, img, point, px, rgb, size, App, AppContext, Application, Bounds, Context, Corners, ExternalPaths,
    FontWeight, Image, ImageFormat, InteractiveElement, IntoElement, KeyBinding, Menu, MenuItem, MouseButton,
    MouseDownEvent, MouseMoveEvent, ParentElement, PathPromptOptions, Pixels, Point, Render, RenderImage,
    ScrollWheelEvent, SharedString, StatefulInteractiveElement, Styled, Task, TitlebarOptions,
    Window, WindowBounds, WindowOptions,
};
use scene::{Camera, Scene, Visibility};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

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

/// The Dock icon, rendered from assets/icon.svg with macOS's icon margin.
#[cfg(target_os = "macos")]
const DOCK_ICON: &[u8] = include_bytes!("../assets/icon-macos.png");

enum State {
    Empty,
    Working(Arc<Mutex<Progress>>),
    Failed { message: String, files: Vec<String> },
    Done(Box<Done>),
}

struct Done {
    conversion: Conversion,
    out: PathBuf,
    /// Rasterized plan preview per storey.
    plans: Vec<(String, Option<Arc<RenderImage>>)>,
    scene: Arc<Scene>,
    /// Programs from the config, for "In PAPA öffnen" and "In cadwork öffnen";
    /// without one they are looked for, or the user picks them.
    papa: Option<PathBuf>,
    cadwork: Option<PathBuf>,
}

/// What the background conversion is doing, written by it and shown while it runs.
struct Progress {
    step: String,
    /// E.g. "Plan 2 von 5".
    detail: Option<String>,
    /// Steps finished of `total`: one per plan file, saving and the previews.
    done: usize,
    total: usize,
}

impl Progress {
    fn set(&mut self, step: impl Into<String>, detail: Option<String>, done: usize) {
        self.step = step.into();
        self.detail = detail;
        self.done = done;
    }
}

/// A newer release on GitHub, shown in a bar under the header.
enum AppUpdate {
    Available(update::Release),
    Installing(String),
    Installed { version: String, exe: PathBuf },
    Failed(String),
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
    app_update: Option<AppUpdate>,
    _task: Option<Task<()>>,
    _progress_task: Option<Task<()>>,
    _update_task: Option<Task<()>>,
}

impl Converter {
    fn start(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        if paths.is_empty() {
            return;
        }
        let progress = Arc::new(Mutex::new(Progress { step: "Pläne werden gesucht …".into(), detail: None, done: 0, total: 1 }));
        self.state = State::Working(progress.clone());
        self.notice = None;
        let job = cx.background_executor().spawn(async move { run(paths, &progress) });
        // The conversion cannot notify the view from its thread; redraw until it is done.
        self._progress_task = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(100)).await;
            let working = this.update(cx, |this, cx| {
                cx.notify();
                matches!(this.state, State::Working(_))
            });
            if !matches!(working, Ok(true)) {
                break;
            }
        }));
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

    /// Finds the program, or asks for it when it can't be found.
    fn open_program(&mut self, program: Program, cx: &mut Context<Self>) {
        let State::Done(done) = &self.state else { return };
        let configured = match program {
            Program::Papa => done.papa.clone(),
            Program::Cadwork => done.cadwork.clone(),
        };
        match programs::find(program, configured.as_deref()) {
            Some(exe) => self.open_in(program, Some(exe), cx),
            None => self.pick_program(program, cx),
        }
    }

    /// Lets the user show where the program is; it is kept for the next time.
    fn pick_program(&mut self, program: Program, cx: &mut Context<Self>) {
        let name = program.name();
        self.notice = Some(format!("{name} wurde nicht gefunden. Bitte das Programm auswählen ({}), Aufmaß merkt es sich.", program.exe_hint()));
        cx.notify();
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(format!("{name} auswählen").into()),
        });
        cx.spawn(async move |this, cx| {
            let exe = match picked.await {
                Ok(Ok(Some(mut paths))) => paths.pop(),
                _ => None,
            };
            let saved = exe.as_ref().map(|e| programs::remember(program, e));
            this.update(cx, |this, cx| {
                this.open_in(program, exe, cx);
                if let (Some(Err(e)), Some(notice)) = (saved, &mut this.notice) {
                    notice.push_str(&format!(" Die Auswahl konnte nicht gespeichert werden: {e:#}"));
                }
            })
            .ok();
        })
        .detach();
    }

    fn open_in(&mut self, program: Program, exe: Option<PathBuf>, cx: &mut Context<Self>) {
        match program {
            Program::Papa => self.open_in_papa(exe, cx),
            Program::Cadwork => self.open_in_cadwork(exe, cx),
        }
    }

    /// Starts PAPA and puts the GAEB file path on the clipboard; PAPA has no
    /// way to be handed a file to import, so the user pastes it in the GAEB import.
    fn open_in_papa(&mut self, exe: Option<PathBuf>, cx: &mut Context<Self>) {
        let State::Done(done) = &self.state else { return };
        let file = done.out.join(GAEB_FILE);
        cx.write_to_clipboard(ClipboardItem::new_string(file.display().to_string()));
        let started = exe.map(|exe| launch(&exe, "PAPA"));
        self.notice = Some(match started {
            Some(Ok(_)) => "PAPA startet. Der Pfad zu mengen.x83 ist kopiert: in PAPA das Projekt öffnen, GAEB-Import wählen und den Pfad einfügen (Strg+V).".into(),
            Some(Err(e)) => {
                cx.reveal_path(&file);
                format!("{e} Der Ordner mit mengen.x83 ist geöffnet, der Pfad kopiert.")
            }
            None => {
                cx.reveal_path(&file);
                "Kein PAPA-Programm ausgewählt. Der Ordner mit mengen.x83 ist geöffnet, der Pfad kopiert.".into()
            }
        });
        cx.notify();
    }

    /// Installs the import plugin into cadwork and starts cadwork; the plugin
    /// picks up this result on its own, so the user only runs it there.
    fn open_in_cadwork(&mut self, exe: Option<PathBuf>, cx: &mut Context<Self>) {
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
        self.notice = Some(match exe.map(|exe| launch(&exe, "cadwork")) {
            Some(Ok(())) => format!("cadwork startet. {run}{manual}"),
            Some(Err(e)) => format!("{e} {run}{manual}"),
            None => format!("Kein cadwork-Programm ausgewählt. {run}{manual}"),
        });
        cx.notify();
    }

    fn check_for_update(&mut self, cx: &mut Context<Self>) {
        let job = cx.background_executor().spawn(async { update::check() });
        self._update_task = Some(cx.spawn(async move |this, cx| {
            // Offline or GitHub unreachable: asked again on the next start.
            if let Ok(Some(release)) = job.await {
                this.update(cx, |this, cx| {
                    this.app_update = Some(AppUpdate::Available(release));
                    cx.notify();
                })
                .ok();
            }
        }));
    }

    fn install_update(&mut self, cx: &mut Context<Self>) {
        let Some(AppUpdate::Available(release)) = self.app_update.take() else { return };
        self.app_update = Some(AppUpdate::Installing(release.version.clone()));
        let job = cx.background_executor().spawn(async move { (update::install(&release), release.version) });
        self._update_task = Some(cx.spawn(async move |this, cx| {
            let (result, version) = job.await;
            this.update(cx, |this, cx| {
                this.app_update = Some(match result {
                    Ok(exe) => AppUpdate::Installed { version, exe },
                    Err(e) => {
                        telemetry::update_failed(&e);
                        AppUpdate::Failed(format!("Update auf {version} fehlgeschlagen: {e:#}"))
                    }
                });
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// Starts the new version and closes this one; the results are saved already.
    fn restart(&mut self, exe: PathBuf, cx: &mut Context<Self>) {
        match std::process::Command::new(&exe).spawn() {
            Ok(_) => cx.quit(),
            Err(e) => {
                self.app_update = Some(AppUpdate::Failed(format!("Aufmaß konnte nicht neu gestartet werden: {e}. Bitte von Hand neu starten.")));
                cx.notify();
            }
        }
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
fn run(paths: Vec<PathBuf>, progress: &Mutex<Progress>) -> State {
    let report = |step: String, detail: Option<String>, done: usize| progress.lock().unwrap().set(step, detail, done);
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
    let n = files.len();
    progress.lock().unwrap().total = n + 2;
    let conversion = convert_with_progress(&files, &cfg, |i, file| {
        let name = file.file_name().unwrap_or_default().to_string_lossy();
        report(format!("Lese {name} …"), (n > 1).then(|| format!("Plan {} von {n}", i + 1)), i);
    });
    let lines: Vec<String> = conversion.files.iter().map(FileReport::line).collect();
    if let Err(e) = conversion.check() {
        telemetry::conversion(&conversion.files, Some(&e));
        return fail(e, lines);
    }
    report("Ergebnisse werden gespeichert …".into(), Some(format!("mengen.xlsx und {GAEB_FILE}")), n);
    if let Err(e) = conversion.write(&out) {
        let e = e.context("Ergebnis konnte nicht gespeichert werden");
        telemetry::conversion(&conversion.files, Some(&e));
        return fail(e, lines);
    }
    telemetry::conversion(&conversion.files, None);
    report("Vorschau wird vorbereitet …".into(), Some("Grundrisse und 3D-Modell".into()), n + 1);
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
            State::Working(progress) => working(&progress.lock().unwrap()).into_any_element(),
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
            .when_some(self.update_bar(cx), |d, bar| d.child(bar))
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
                        row.child(button("open-papa", "In PAPA öffnen", true).on_click(cx.listener(|this, _, _, cx| this.open_program(Program::Papa, cx))))
                            .child(button("open-cadwork", "In cadwork öffnen", true).on_click(cx.listener(|this, _, _, cx| this.open_program(Program::Cadwork, cx))))
                            .child(button("open-xlsx", "Excel öffnen", false).on_click(cx.listener(move |_, _, _, cx| cx.open_with_system(&xlsx))))
                            .child(button("reveal", "Ordner zeigen", false).on_click(cx.listener(move |_, _, _, cx| cx.reveal_path(&out))))
                    })
                    .child(button("pick-dir", "Ordner wählen …", false).on_click(cx.listener(|this, _, _, cx| this.choose(true, cx))))
                    .child(button("pick-files", "Dateien wählen …", false).on_click(cx.listener(|this, _, _, cx| this.choose(false, cx)))),
            )
    }

    fn update_bar(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let bar = div().flex().items_center().gap_3().px_4().py_2().bg(rgb(ACCENT_SOFT)).border_b_1().border_color(rgb(BORDER));
        let dismiss = || {
            div().id("dismiss-update").text_color(rgb(MUTED)).cursor_pointer().child("✕").on_click(cx.listener(|this, _, _, cx| {
                this.app_update = None;
                cx.notify();
            }))
        };
        Some(match self.app_update.as_ref()? {
            AppUpdate::Available(release) => bar
                .child(div().flex_1().child(format!("Aufmaß {} ist verfügbar (installiert: {}).", release.version, update::VERSION)))
                .child(button("install-update", "Jetzt aktualisieren", true).on_click(cx.listener(|this, _, _, cx| this.install_update(cx))))
                .child(dismiss()),
            AppUpdate::Installing(version) => bar.child(div().flex_1().text_color(rgb(MUTED)).child(format!("Aufmaß {version} wird geladen …"))),
            AppUpdate::Installed { version, exe } => {
                let exe = exe.clone();
                bar.child(div().flex_1().child(format!("Aufmaß {version} ist installiert und startet nach einem Neustart.")))
                    .child(button("restart", "Neu starten", true).on_click(cx.listener(move |this, _, _, cx| this.restart(exe.clone(), cx))))
                    .child(dismiss())
            }
            AppUpdate::Failed(message) => bar.child(div().flex_1().text_color(rgb(DANGER)).child(message.clone())).child(dismiss()),
        })
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
                    .gap(px(8.))
                    .px_3()
                    .py_2()
                    .children(toggles)
                    .child(toggle("roof", "Dach", self.visibility.roof).on_click(cx.listener(|this, _, _, cx| {
                        this.visibility.roof = !this.visibility.roof;
                        cx.notify();
                    })))
                    .child(button("reset-view", "Ansicht zurücksetzen", false).h(px(TOGGLE_HEIGHT)).py_0().flex().items_center().text_xs().on_click(cx.listener(|this, _, _, cx| {
                        this.camera = Camera::default();
                        cx.notify();
                    })))
                    .child(div().flex_1().flex().justify_end().text_xs().text_color(rgb(MUTED)).child("Ziehen: drehen · Scrollen: zoomen")),
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
            div().flex().flex_wrap().gap(px(8.)).mt_2().children(chips.enumerate().map(|(k, (storey, label))| {
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

/// One line of the quantity list; `total` lines are bold and leave the area
/// empty, see [`report::subtotal`].
fn quantity_row(name: String, t: &report::Sum, total: bool) -> impl IntoElement {
    div()
        .flex()
        .gap_2()
        .py_1()
        .when(!total, |d| d.border_b_1().border_color(rgb(SUNKEN)))
        .when(total, |d| d.font_weight(FontWeight::SEMIBOLD))
        .child(div().flex_1().min_w_0().child(name))
        .child(div().w(px(36.)).text_right().when(!total, |d| d.text_color(rgb(MUTED))).child(t.count.to_string()))
        .child(div().w(px(88.)).text_right().when(!total, |d| d.child(format!("{} m²", de(t.net, 2)))))
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

/// Height of toggles and the buttons next to them, so a row lines up.
const TOGGLE_HEIGHT: f32 = 28.;

fn toggle(id: impl Into<gpui::ElementId>, label: impl Into<SharedString>, on: bool) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .h(px(TOGGLE_HEIGHT))
        .min_w(px(56.))
        .px_2p5()
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .border_1()
        .cursor_pointer()
        .text_xs()
        .when(on, |d| d.bg(rgb(ACCENT_SOFT)).border_color(rgb(ACCENT)).text_color(rgb(ACCENT)))
        .when(!on, |d| d.bg(rgb(PANEL)).border_color(rgb(BORDER)).text_color(rgb(MUTED)))
        .child(label.into())
}

#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)] // objc 0.2's macros test a `cargo-clippy` feature
fn set_dock_icon() {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};
    unsafe {
        let data: *mut Object = msg_send![class!(NSData), dataWithBytes: DOCK_ICON.as_ptr() length: DOCK_ICON.len()];
        let image: *mut Object = msg_send![class!(NSImage), alloc];
        let image: *mut Object = msg_send![image, initWithData: data];
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let () = msg_send![app, setApplicationIconImage: image];
    }
}

/// The current step, a bar with one segment per step whose current one pulses, and how far along.
fn working(p: &Progress) -> impl IntoElement {
    let segment = 1. / p.total as f32;
    let bar = div()
        .relative()
        .w(px(320.))
        .h(px(6.))
        .rounded_full()
        .overflow_hidden()
        .bg(rgb(SUNKEN))
        .child(div().absolute().top_0().left_0().h_full().w(relative(p.done as f32 * segment)).bg(rgb(ACCENT)))
        .child(
            div().absolute().top_0().h_full().left(relative(p.done as f32 * segment)).w(relative(segment)).bg(rgb(ACCENT)).with_animation(
                "working-step",
                Animation::new(Duration::from_millis(1200)).repeat().with_easing(gpui::pulsating_between(0.15, 0.6)),
                |el, delta| el.opacity(delta),
            ),
        );
    centered(
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap_3()
            .child(div().font_weight(FontWeight::MEDIUM).child(p.step.clone()))
            .child(bar)
            .child(div().h_4().text_xs().text_color(rgb(MUTED)).children(p.detail.clone())),
    )
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
        #[cfg(target_os = "macos")]
        set_dock_icon();
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
                        app_update: None,
                        _task: None,
                        _progress_task: None,
                        _update_task: None,
                    };
                    view.start(paths, cx);
                    // Development builds would offer to replace themselves with the
                    // release; they only check a test server (AUFMASS_RELEASES_API).
                    if !cfg!(debug_assertions) || std::env::var_os("AUFMASS_RELEASES_API").is_some() {
                        view.check_for_update(cx);
                    }
                    view
                })
            },
        )
        .expect("opening the window failed");
        cx.activate(true);
    });
}
