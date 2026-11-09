//! 平台输入 → egui 事件。首版移植聚焦指针/触摸/滚轮/键盘；手写笔（pen）留作后续工作。
use egui::{Event, ImeEvent, Key, Modifiers, PointerButton, Pos2, TouchDeviceId, TouchId, TouchPhase};
use serde::Deserialize;
use std::collections::BTreeMap;

/// 平台坐标（物理像素）在 ArkTS 侧上报；egui 事件用点（除以 scale）。
#[derive(Deserialize, Default)]
pub struct Contact {
    pub id: u64,
    pub x: f32,
    pub y: f32,
}

#[derive(Deserialize, Default)]
pub struct Input {
    pub kind: String,
    #[serde(default)]
    pub points: Vec<Contact>,
    #[serde(default)]
    pub zoom: f32,
    #[serde(default)]
    pub dx: f32,
    #[serde(default)]
    pub dy: f32,
    #[serde(default)]
    pub action: i32, // 0 down, 1 up, 2 move, 3 cancel
    #[serde(default)]
    pub x: f32,
    #[serde(default)]
    pub y: f32,
    #[serde(default)]
    pub button: i32,
    #[serde(default)]
    pub code: i32,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub ctrl: bool,
    #[serde(default)]
    pub shift: bool,
    #[serde(default)]
    pub alt: bool,
    /// IME 子操作：commit / preedit / delete / enter（仅 kind == "ime" 时使用）。
    #[serde(default)]
    pub op: String,
}

#[derive(Default)]
pub struct State {
    pressed: Option<PointerButton>,
    last_pointer: Option<Pos2>,
    touches: BTreeMap<u64, Pos2>,
    touch_pointer: Option<u64>,
    pub modifiers: Modifiers,
}

impl State {
    fn move_pointer(&mut self, pos: Pos2, events: &mut Vec<Event>) {
        self.last_pointer = Some(pos);
        events.push(Event::PointerMoved(pos));
    }

    pub fn cancel(&mut self, events: &mut Vec<Event>) {
        let release = self.pressed.take().zip(self.last_pointer.take());
        self.touches.clear();
        self.touch_pointer = None;
        self.modifiers = Modifiers::NONE;
        if let Some((button, pos)) = release {
            events.push(Event::PointerButton {
                pos,
                button,
                pressed: false,
                modifiers: self.modifiers,
            });
        }
        events.push(Event::PointerGone);
    }

    pub fn push(&mut self, input: Input, scale: f32, events: &mut Vec<Event>) {
        if input.kind == "blur" {
            self.cancel(events);
            return;
        }
        if (!input.x.is_finite() || !input.y.is_finite()) || !scale.is_finite() || scale <= 0.0 {
            return;
        }
        if input.kind == "key" {
            let old = self.modifiers;
            self.modifiers = Modifiers {
                ctrl: input.ctrl,
                shift: input.shift,
                alt: input.alt,
                command: input.ctrl,
                ..Default::default()
            };
            if old != self.modifiers {
                events.push(Event::ModifiersChanged(self.modifiers));
            }
        }
        let pos = Pos2::new(input.x / scale, input.y / scale);
        match input.kind.as_str() {
            "touch" => {
                let next: BTreeMap<_, _> = input
                    .points
                    .iter()
                    .map(|p| (p.id, Pos2::new(p.x / scale, p.y / scale)))
                    .collect();
                for (&id, &p) in &self.touches {
                    if !next.contains_key(&id) {
                        events.push(Event::Touch {
                            device_id: TouchDeviceId(0),
                            id: TouchId(id),
                            phase: TouchPhase::End,
                            pos: p,
                            force: None,
                        });
                        if self.touch_pointer == Some(id) {
                            if let Some(button) = self.pressed.take() {
                                events.push(Event::PointerButton {
                                    pos: p,
                                    button,
                                    pressed: false,
                                    modifiers: self.modifiers,
                                });
                            }
                            events.push(Event::PointerGone);
                            self.touch_pointer = None;
                        }
                    }
                }
                let multi = next.len() >= 2;
                if multi && (self.touch_pointer.is_some() || self.pressed.is_some()) {
                    let anchor = self
                        .touch_pointer
                        .and_then(|id| self.touches.get(&id).copied())
                        .or_else(|| next.values().next().copied())
                        .unwrap_or(Pos2::ZERO);
                    if let Some(button) = self.pressed.take() {
                        events.push(Event::PointerButton {
                            pos: anchor,
                            button,
                            pressed: false,
                            modifiers: self.modifiers,
                        });
                    }
                    if self.touch_pointer.take().is_some() {
                        events.push(Event::PointerGone);
                    }
                }
                let _first = self.touches.is_empty();
                for (&id, &p) in &next {
                    let phase = if self.touches.contains_key(&id) {
                        TouchPhase::Move
                    } else {
                        TouchPhase::Start
                    };
                    events.push(Event::Touch {
                        device_id: TouchDeviceId(0),
                        id: TouchId(id),
                        phase,
                        pos: p,
                        force: None,
                    });
                    if self.touch_pointer == Some(id) {
                        self.move_pointer(p, events);
                    }
                }
                // 单指视为主指针按下；多指只做导航（此处仅跟踪，不注入主指针）。
                if !multi && next.len() == 1 {
                    if let Some((&id, &p)) = next.iter().next() {
                        if self.touch_pointer.is_none() && self.pressed.is_none() {
                            self.touch_pointer = Some(id);
                            self.pressed = Some(PointerButton::Primary);
                            self.move_pointer(p, events);
                            events.push(Event::PointerButton {
                                pos: p,
                                button: PointerButton::Primary,
                                pressed: true,
                                modifiers: self.modifiers,
                            });
                        }
                    }
                }
                self.touches = next;
            }
            "pointer" => {
                let button = match input.button {
                    1 => PointerButton::Secondary,
                    2 => PointerButton::Middle,
                    _ => PointerButton::Primary,
                };
                self.move_pointer(pos, events);
                match input.action {
                    0 if self.pressed.is_none() => {
                        self.pressed = Some(button);
                        events.push(Event::PointerButton {
                            pos,
                            button,
                            pressed: true,
                            modifiers: self.modifiers,
                        });
                    }
                    1 => {
                        if let Some(b) = self.pressed.take() {
                            events.push(Event::PointerButton {
                                pos,
                                button: b,
                                pressed: false,
                                modifiers: self.modifiers,
                            });
                        }
                        events.push(Event::PointerGone);
                    }
                    3 => self.cancel(events),
                    _ => {}
                }
            }
            "axis" => {
                if input.zoom.is_finite() && input.zoom > 0.0 {
                    events.push(Event::Zoom(input.zoom));
                } else if input.dx != 0.0 || input.dy != 0.0 {
                    events.push(Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        phase: TouchPhase::Move,
                        delta: egui::vec2(input.dx / scale, input.dy / scale),
                        modifiers: self.modifiers,
                        source: egui::MouseWheelSource::Unknown,
                    });
                }
            }
            "wheel" => events.push(Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                phase: TouchPhase::Move,
                delta: egui::vec2(input.x, input.y),
                modifiers: self.modifiers,
                source: egui::MouseWheelSource::Unknown,
            }),
            "key" => {
                let pressed = input.action == 0;
                if let Some(key) = key(input.code) {
                    events.push(Event::Key {
                        key,
                        physical_key: Some(key),
                        pressed,
                        repeat: false,
                        modifiers: self.modifiers,
                    });
                }
                if pressed
                    && !input.ctrl
                    && !input.alt
                    && !input.text.is_empty()
                    && !input.text.chars().any(char::is_control)
                {
                    events.push(Event::Text(input.text));
                }
            }
            "ime" => {
                // 软键盘中文输入法经 HarmonyOS InputMethod 框架回传，不经原始按键事件：
                // commit = 已确认上屏的中文字符；preedit = 拼音组合态；delete = IME 退格；enter = 回车。
                match input.op.as_str() {
                    "commit" => {
                        if !input.text.is_empty() {
                            events.push(Event::Ime(ImeEvent::Commit(input.text.clone())));
                        }
                    }
                    "preedit" => {
                        // 空串表示清除组合态（egui 收到空 Preedit 会清掉已有组合）。
                        events.push(Event::Ime(ImeEvent::Preedit {
                            text: input.text.clone(),
                            active_range_chars: None,
                        }));
                    }
                    "delete" => {
                        if let Ok(n) = input.text.parse::<u32>() {
                            for _ in 0..n.min(64) {
                                for &pressed in &[true, false] {
                                    events.push(Event::Key {
                                        key: Key::Backspace,
                                        physical_key: Some(Key::Backspace),
                                        pressed,
                                        repeat: false,
                                        modifiers: self.modifiers,
                                    });
                                }
                            }
                        }
                    }
                    "enter" => {
                        for &pressed in &[true, false] {
                            events.push(Event::Key {
                                key: Key::Enter,
                                physical_key: Some(Key::Enter),
                                pressed,
                                repeat: false,
                                modifiers: self.modifiers,
                            });
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

fn key(code: i32) -> Option<Key> {
    const LETTERS: [Key; 26] = [
        Key::A, Key::B, Key::C, Key::D, Key::E, Key::F, Key::G, Key::H, Key::I, Key::J, Key::K, Key::L,
        Key::M, Key::N, Key::O, Key::P, Key::Q, Key::R, Key::S, Key::T, Key::U, Key::V, Key::W, Key::X,
        Key::Y, Key::Z,
    ];
    const DIGITS: [Key; 10] = [
        Key::Num0, Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6, Key::Num7, Key::Num8,
        Key::Num9,
    ];
    if (2017..=2042).contains(&code) {
        return LETTERS.get((code - 2017) as usize).copied();
    }
    if (2000..=2009).contains(&code) {
        return DIGITS.get((code - 2000) as usize).copied();
    }
    Some(match code {
        2012 => Key::ArrowUp,
        2013 => Key::ArrowDown,
        2014 => Key::ArrowLeft,
        2015 => Key::ArrowRight,
        2049 => Key::Tab,
        2050 => Key::Space,
        2054 => Key::Enter,
        2055 => Key::Backspace,
        2068 => Key::PageUp,
        2069 => Key::PageDown,
        2070 => Key::Escape,
        2071 => Key::Delete,
        2081 => Key::Home,
        2082 => Key::End,
        _ => return None,
    })
}
