//! La ventana de jale: el menú dibujado con el theme de rofi del look
//! (view.rs), en medio de la pantalla y en la capa de encima, con el teclado
//! para ella. Arriba lo que escribes, abajo la lista filtrada al vuelo.
//!
//!   escribir filtra · ↑↓ Tab Ctrl+J/K eligen (←→ también, en cuadrícula)
//!   RePág/AvPág saltan · Enter abre · Shift+Enter manda lo escrito tal cual
//!   Ctrl+U borra todo · Ctrl+W o Ctrl+Retroceso, la última palabra · Esc cierra
//!   el mouse: pasar elige (hover-select), clic abre, la rueda baja y sube

use crate::fuzzy;
use crate::rasi;
use crate::view::{Laid, State, View};
use crate::{Item, Opts, Outcome};
use smithay_client_toolkit::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay_client_toolkit::reexports::calloop::{EventLoop, LoopHandle};
use smithay_client_toolkit::reexports::calloop_wayland_source::WaylandSource;
use smithay_client_toolkit::reexports::client::globals::registry_queue_init;
use smithay_client_toolkit::reexports::client::protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_shm, wl_surface};
use smithay_client_toolkit::reexports::client::{Connection, QueueHandle};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_registry,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        Capability, SeatHandler, SeatState,
        keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers, RawModifiers},
        pointer::{PointerEvent, PointerEventKind, PointerHandler},
    },
    shell::{
        WaylandSurface,
        wlr_layer::{KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface, LayerSurfaceConfigure},
    },
    shm::{Shm, ShmHandler, slot::SlotPool},
};
use std::time::Duration;
use tiny_skia::Pixmap;

const BTN_LEFT: u32 = 0x110;
/// El parpadeo del cursor (si el theme dice `blink: true`).
const BLINK: Duration = Duration::from_millis(530);

struct App {
    registry: RegistryState,
    seat: SeatState,
    output: OutputState,
    compositor: CompositorState,
    layer_shell: LayerShell,
    shm: Shm,
    handle: LoopHandle<'static, App>,
    pool: Option<SlotPool>,
    layer: Option<LayerSurface>,
    configured: bool,
    scale: i32,
    /// el tamaño pedido a la superficie (px lógicos)
    size: (u32, u32),
    keyboard: Option<wl_keyboard::WlKeyboard>,
    pointer: Option<wl_pointer::WlPointer>,
    ctrl: bool,
    shift: bool,
    alt: bool,
    logo: bool,
    /// dónde entró el puntero: hasta que se mueva de ahí no elige nada (si
    /// jale abre bajo el mouse, la selección no debe brincar sola)
    pointer_at: Option<(f64, f64)>,
    hover_select: bool,

    items: Vec<Item>,
    /// el orden sin búsqueda (lo más usado primero), por renglón
    order: Vec<f64>,
    opts: Opts,
    view: View,
    mon_w: f32,
    query: String,
    /// los renglones que pasan el filtro, ya ordenados
    matches: Vec<usize>,
    sel: usize,
    /// el primer renglón (de la cuadrícula) que se ve
    top: usize,
    cursor_on: bool,
    /// la forma de la última vez: columnas, renglones y dónde cayó cada uno
    laid: Laid,
    outcome: Option<Outcome>,
}

impl App {
    /// Medir (sin `pix`) o dibujar el menú como está ahora.
    fn layout(&mut self, pix: Option<&mut Pixmap>, s: f32) -> Laid {
        let App { view, items, matches, sel, top, query, opts, cursor_on, mon_w, .. } = self;
        let st = State {
            items,
            matches,
            sel: *sel,
            top: *top,
            query,
            prompt: &opts.prompt,
            mesg: opts.mesg.as_deref(),
            mode: opts.mode.as_deref(),
            cursor_on: *cursor_on,
            lines: opts.lines,
        };
        view.layout(&st, *mon_w, pix, s)
    }

    fn filter(&mut self) {
        let q: Vec<char> = self.query.to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
        let mut m: Vec<(i32, usize)> = self.items.iter().enumerate().filter_map(|(i, it)| fuzzy::score(&q, &it.search).map(|s| (s, i))).collect();
        // con búsqueda manda el parecido (y un empujón a lo más usado); sin
        // ella, lo más usado y luego como llegaron
        let order = &self.order;
        m.sort_by(|a, b| {
            let ka = a.0 as f64 + order.get(a.1).copied().unwrap_or(0.0).min(20.0);
            let kb = b.0 as f64 + order.get(b.1).copied().unwrap_or(0.0).min(20.0);
            kb.partial_cmp(&ka).unwrap_or(std::cmp::Ordering::Equal).then(a.1.cmp(&b.1))
        });
        if q.is_empty() && order.iter().all(|&o| o == 0.0) {
            m.sort_by_key(|x| x.1);
        }
        self.matches = m.into_iter().map(|x| x.1).collect();
        self.sel = 0;
        self.top = 0;
    }

    fn cols(&self) -> usize {
        self.laid.columns.max(1)
    }

    fn move_sel(&mut self, d: isize) {
        if self.matches.is_empty() {
            return;
        }
        let n = self.matches.len() as isize;
        self.sel = (self.sel as isize + d).rem_euclid(n) as usize;
        let (row, lines) = (self.sel / self.cols(), self.laid.lines.max(1));
        if row < self.top {
            self.top = row;
        } else if row >= self.top + lines {
            self.top = row + 1 - lines;
        }
        self.refresh();
    }

    fn accept(&mut self, custom: bool) {
        self.outcome = Some(match self.matches.get(self.sel) {
            Some(&i) if !custom => Outcome::Pick(i),
            _ if !self.opts.no_custom && !self.query.is_empty() => Outcome::Custom(self.query.clone()),
            _ => return,
        });
    }

    /// ¿La tecla es la combinación de rofi `combo` ("Control-Delete")?
    fn is_combo(&self, combo: &str, ev: &KeyEvent) -> bool {
        let mut parts: Vec<&str> = combo.split('-').collect();
        let Some(key) = parts.pop() else { return false };
        let want = |m: &str| parts.iter().any(|p| p.eq_ignore_ascii_case(m));
        let mods_ok = want("Control") == self.ctrl && want("Alt") == self.alt && want("Shift") == self.shift && want("Super") == self.logo;
        let sym = match key {
            "Delete" => Some(Keysym::Delete),
            "BackSpace" => Some(Keysym::BackSpace),
            "Return" => Some(Keysym::Return),
            "Tab" => Some(Keysym::Tab),
            "space" => Some(Keysym::space),
            _ => None,
        };
        let key_ok = match sym {
            Some(k) => ev.keysym == k,
            // una letra o número: lo que escribe la tecla
            None => key.chars().count() == 1 && ev.keysym.key_char().is_some_and(|c| c.eq_ignore_ascii_case(&key.chars().next().unwrap())),
        };
        mods_ok && key_ok
    }

    fn key(&mut self, ev: KeyEvent) {
        if let Some(&(n, _)) = self.opts.custom_keys.iter().find(|(_, combo)| self.is_combo(combo, &ev)) {
            self.outcome = Some(Outcome::Key(n, self.matches.get(self.sel).copied()));
            return;
        }
        self.cursor_on = true;
        let (cols, page) = (self.cols() as isize, (self.laid.lines.max(1) * self.cols()) as isize);
        match ev.keysym {
            Keysym::Escape => self.outcome = Some(Outcome::Cancel),
            Keysym::Return | Keysym::KP_Enter => self.accept(self.shift),
            Keysym::Up => self.move_sel(-cols),
            Keysym::Down => self.move_sel(cols),
            Keysym::Left if cols > 1 => self.move_sel(-1),
            Keysym::Right if cols > 1 => self.move_sel(1),
            Keysym::ISO_Left_Tab => self.move_sel(-1),
            Keysym::Tab => self.move_sel(1),
            Keysym::k | Keysym::p if self.ctrl => self.move_sel(-cols),
            Keysym::j | Keysym::n if self.ctrl => self.move_sel(cols),
            Keysym::Page_Up => self.move_sel(-page),
            Keysym::Page_Down => self.move_sel(page),
            Keysym::u if self.ctrl => {
                self.query.clear();
                self.refilter();
            }
            Keysym::w if self.ctrl => self.delete_word(),
            Keysym::BackSpace if self.ctrl => self.delete_word(),
            Keysym::BackSpace => {
                self.query.pop();
                self.refilter();
            }
            _ => {
                if self.ctrl || self.alt || self.logo {
                    return;
                }
                if let Some(t) = ev.utf8.filter(|t| t.chars().all(|c| !c.is_control())) {
                    self.query.push_str(&t);
                    self.refilter();
                }
            }
        }
    }

    fn delete_word(&mut self) {
        let trimmed = self.query.trim_end().len();
        self.query.truncate(trimmed);
        let cut = self.query.rfind(char::is_whitespace).map_or(0, |i| i + 1);
        self.query.truncate(cut);
        self.refilter();
    }

    fn refilter(&mut self) {
        self.filter();
        self.refresh();
    }

    /// Volver a medir: si cambió el tamaño, pedírselo al compositor (su
    /// configure dibuja); si no, dibujar de una vez.
    fn refresh(&mut self) {
        let laid = self.layout(None, 1.0);
        self.laid.columns = laid.columns;
        self.laid.lines = laid.lines;
        let size = (laid.w.ceil() as u32, laid.h.ceil() as u32);
        if size != self.size {
            self.size = size;
            if let Some(layer) = &self.layer {
                layer.set_size(size.0, size.1);
                layer.commit();
            }
            self.configured = false;
            return;
        }
        self.draw();
    }

    fn draw(&mut self) {
        let (Some(layer), true) = (self.layer.clone(), self.configured) else { return };
        let s = self.scale.max(1);
        let (w, h) = (self.size.0 as i32 * s, self.size.1 as i32 * s);
        let Some(mut pix) = Pixmap::new(w.max(1) as u32, h.max(1) as u32) else { return };
        self.laid = self.layout(Some(&mut pix), s as f32);

        let pool = match &mut self.pool {
            Some(p) => p,
            None => self.pool.insert(SlotPool::new((w * h * 4).max(4) as usize, &self.shm).expect("memoria compartida")),
        };
        let Ok((buffer, canvas)) = pool.create_buffer(w, h, w * 4, wl_shm::Format::Argb8888) else { return };
        for (dst, src) in canvas.chunks_exact_mut(4).zip(pix.data().chunks_exact(4)) {
            dst[0] = src[2];
            dst[1] = src[1];
            dst[2] = src[0];
            dst[3] = src[3];
        }
        let surface = layer.wl_surface();
        surface.set_buffer_scale(s);
        surface.damage_buffer(0, 0, w, h);
        if buffer.attach_to(surface).is_ok() {
            layer.commit();
        }
    }

    /// El renglón (posición en `matches`) bajo el puntero.
    fn hit(&self, x: f64, y: f64) -> Option<usize> {
        self.laid.hits.iter().find(|(r, _)| r.contains(x as f32, y as f32)).map(|(_, pos)| *pos)
    }
}

/// El ancho lógico del monitor enfocado (para los `width: 50%`).
fn monitor_width() -> f32 {
    std::process::Command::new("hyprctl")
        .args(["monitors", "-j"])
        .output()
        .ok()
        .and_then(|o| serde_json::from_slice::<serde_json::Value>(&o.stdout).ok())
        .and_then(|v| {
            let m = v.as_array()?.iter().find(|m| m["focused"].as_bool() == Some(true))?.clone();
            Some(m["width"].as_f64()? as f32 / m["scale"].as_f64().unwrap_or(1.0) as f32)
        })
        .unwrap_or(1920.0)
}

pub fn run(items: Vec<Item>, order: Vec<f64>, opts: Opts) -> Result<Outcome, String> {
    let conn = Connection::connect_to_env().map_err(|e| format!("Wayland: {e}"))?;
    let (globals, queue) = registry_queue_init(&conn).map_err(|e| format!("Wayland: {e}"))?;
    let qh = queue.handle();
    let mut event_loop: EventLoop<'static, App> = EventLoop::try_new().map_err(|e| e.to_string())?;
    let handle = event_loop.handle();
    WaylandSource::new(conn.clone(), queue).insert(handle.clone()).map_err(|e| e.to_string())?;
    let selected = opts.selected;
    let view = View::new(&opts.config, opts.theme_str.as_deref(), opts.mode.is_some());
    let hover_select = view.theme.config("hover-select").is_none_or(|v| rasi::boolean(&v));
    let blink = view.theme.get(&["window", "mainbox", "inputbar", "entry"], None, "blink").is_some_and(|v| rasi::boolean(&v));

    let mut app = App {
        registry: RegistryState::new(&globals),
        seat: SeatState::new(&globals, &qh),
        output: OutputState::new(&globals, &qh),
        compositor: CompositorState::bind(&globals, &qh).map_err(|_| "el compositor no tiene wl_compositor")?,
        layer_shell: LayerShell::bind(&globals, &qh).map_err(|_| "el compositor no tiene layer-shell")?,
        shm: Shm::bind(&globals, &qh).map_err(|_| "el compositor no tiene wl_shm")?,
        handle,
        pool: None,
        layer: None,
        configured: false,
        scale: 1,
        size: (0, 0),
        keyboard: None,
        pointer: None,
        ctrl: false,
        shift: false,
        alt: false,
        logo: false,
        pointer_at: None,
        hover_select,
        items,
        order,
        opts,
        view,
        mon_w: monitor_width(),
        query: String::new(),
        matches: Vec::new(),
        sel: 0,
        top: 0,
        cursor_on: true,
        laid: Laid::default(),
        outcome: None,
    };
    app.filter();

    let surface = app.compositor.create_surface(&qh);
    let layer = app.layer_shell.create_layer_surface(&qh, surface, Layer::Overlay, Some("jale"), None);
    layer.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
    app.layer = Some(layer);
    // el primer tamaño (y la forma de la lista, para -selected-row)
    app.refresh();
    if selected > 0 {
        app.move_sel(selected as isize);
    }
    if blink {
        app.handle
            .insert_source(Timer::from_duration(BLINK), |_, _, app: &mut App| {
                app.cursor_on = !app.cursor_on;
                app.draw();
                TimeoutAction::ToDuration(BLINK)
            })
            .map_err(|e| e.to_string())?;
    }

    while app.outcome.is_none() {
        event_loop.dispatch(Duration::from_millis(100), &mut app).map_err(|e| e.to_string())?;
    }
    // que se vaya ya, antes de abrir la app o de devolver al script
    app.layer = None;
    let _ = conn.roundtrip();
    Ok(app.outcome.take().unwrap_or(Outcome::Cancel))
}

impl CompositorHandler for App {
    fn scale_factor_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, factor: i32) {
        if factor != self.scale {
            self.scale = factor;
            self.pool = None;
            self.draw();
        }
    }
    fn transform_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: wl_output::Transform) {}
    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {}
    fn surface_enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
    fn surface_leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl LayerShellHandler for App {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {
        self.outcome = Some(Outcome::Cancel);
    }
    fn configure(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface, _: LayerSurfaceConfigure, _: u32) {
        self.pool = None;
        self.configured = true;
        self.draw();
    }
}

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat
    }
    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
    fn new_capability(&mut self, _: &Connection, qh: &QueueHandle<Self>, seat: wl_seat::WlSeat, cap: Capability) {
        if cap == Capability::Keyboard && self.keyboard.is_none() {
            let cb = Box::new(|app: &mut App, _: &wl_keyboard::WlKeyboard, ev: KeyEvent| app.key(ev));
            self.keyboard = self.seat.get_keyboard_with_repeat(qh, &seat, None, self.handle.clone(), cb).ok();
        }
        if cap == Capability::Pointer && self.pointer.is_none() {
            self.pointer = self.seat.get_pointer(qh, &seat).ok();
        }
    }
    fn remove_capability(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat, _: Capability) {}
    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl KeyboardHandler for App {
    fn enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: &wl_surface::WlSurface, _: u32, _: &[u32], _: &[Keysym]) {}
    fn leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: &wl_surface::WlSurface, _: u32) {}
    fn press_key(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: u32, ev: KeyEvent) {
        self.key(ev);
    }
    fn repeat_key(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: u32, _: KeyEvent) {}
    fn release_key(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: u32, _: KeyEvent) {}
    fn update_modifiers(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: u32, m: Modifiers, _: RawModifiers, _: u32) {
        self.ctrl = m.ctrl;
        self.shift = m.shift;
        self.alt = m.alt;
        self.logo = m.logo;
    }
}

impl PointerHandler for App {
    fn pointer_frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_pointer::WlPointer, events: &[PointerEvent]) {
        for e in events {
            match e.kind {
                PointerEventKind::Enter { .. } => self.pointer_at = Some(e.position),
                PointerEventKind::Motion { .. } => {
                    if self.pointer_at.is_some_and(|p| (p.0 - e.position.0).hypot(p.1 - e.position.1) < 2.0) {
                        continue;
                    }
                    self.pointer_at = None;
                    if self.hover_select
                        && let Some(r) = self.hit(e.position.0, e.position.1)
                        && r != self.sel
                    {
                        self.sel = r;
                        self.draw();
                    }
                }
                PointerEventKind::Press { button: BTN_LEFT, .. } => {
                    if let Some(r) = self.hit(e.position.0, e.position.1) {
                        self.sel = r;
                        self.accept(false);
                    }
                }
                PointerEventKind::Axis { vertical, .. } => {
                    let d = if vertical.discrete != 0 { vertical.discrete.signum() } else { vertical.absolute.signum() as i32 };
                    if d != 0 {
                        self.move_sel(d as isize * self.cols() as isize);
                    }
                }
                _ => {}
            }
        }
    }
}

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

delegate_registry!(App);

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }
    registry_handlers![OutputState, SeatState];
}

smithay_client_toolkit::delegate_dispatch2!(App);
