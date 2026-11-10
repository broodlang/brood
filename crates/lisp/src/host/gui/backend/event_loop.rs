//! The GUI thread's winit event loop: `GuiApp`'s [`ApplicationHandler`] — the user events
//! the Brood side sends through the proxy, every window's input translated into mailbox
//! messages, the paint, and `about_to_wait`'s timers (show timeout, busy cursor, particles,
//! caret blink, wheel backlog, kinetic scroll) — and `run_gui`, which builds and runs it.

use super::*;

/// How often a window with particles in flight repaints by itself (~60 Hz).
const PARTICLE_FRAME: Duration = Duration::from_millis(16);

impl ApplicationHandler<UserEvent> for GuiApp {
    // The loop idles (`Wait`) until a proxy or window event arrives. Window
    // creation is now safe (the display is live), so drain any `Open` that
    // arrived before this fired.
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        event_loop.set_control_flow(ControlFlow::Wait);
        self.resumed = true;
        for (subscriber, spec, reply) in std::mem::take(&mut self.pending_open) {
            self.open_window(event_loop, subscriber, spec, reply);
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        let Some(_gate) = enter_gate() else {
            return;
        };
        match event {
            // Create now if the display is live, else queue until `resumed`.
            UserEvent::Open {
                subscriber,
                spec,
                reply,
            } => {
                if self.resumed {
                    self.open_window(event_loop, subscriber, spec, reply);
                } else {
                    self.pending_open.push((subscriber, spec, reply));
                }
            }
            // Measure a cell at some font size for a window (behind gui-cell-size).
            UserEvent::Metrics { id, px, reply } => {
                let answer = match self.ids.get(&id).and_then(|wid| self.wins.get(wid)) {
                    Some(w) => {
                        let m = match px {
                            Some(p) => w.renderer.metrics_at(p),
                            None => w.renderer.metrics(),
                        };
                        Ok((
                            m.cell_w.min(u16::MAX as usize) as u16,
                            m.cell_h.min(u16::MAX as usize) as u16,
                        ))
                    }
                    None => Err("gui window not open".to_string()),
                };
                let _ = reply.send(answer);
            }
            // Set a live window's OS title-bar text (behind gui-title!).
            UserEvent::Title { id, title } => {
                if let Some(w) = self.ids.get(&id).and_then(|wid| self.wins.get(wid)) {
                    w.window.set_title(&title);
                }
            }
            // Set a live window's taskbar/title-bar icon (behind gui-icon!).
            UserEvent::Icon {
                id,
                rgba,
                w: iw,
                h: ih,
            } => {
                if let Some(win) = self.ids.get(&id).and_then(|wid| self.wins.get(wid))
                    && let Ok(ic) = Icon::from_rgba(rgba, iw, ih)
                {
                    win.window.set_window_icon(Some(ic));
                }
            }
            // A sprite texture lives on the GPU target; the CPU target has no sprite
            // arm, so for it the upload is a no-op (the `:sprite` op is skipped there too).
            UserEvent::Texture {
                id,
                tex,
                rgba,
                w: tw,
                h: th,
            } => {
                if let Some(win) = self.ids.get(&id).and_then(|wid| self.wins.get_mut(wid)) {
                    match &mut win.backend {
                        // The CPU painter samples it from memory (`paint::paint_sprite`).
                        Backend::Cpu { .. } => {
                            win.renderer
                                .textures
                                .insert(tex, render::CpuTexture { w: tw, h: th, rgba });
                        }
                        #[cfg(feature = "gui-gpu")]
                        Backend::Gpu(gpu) => gpu.upload_texture(tex, &rgba, tw, th),
                    }
                }
            }
            UserEvent::TextureFree { id, tex } => {
                if let Some(win) = self.ids.get(&id).and_then(|wid| self.wins.get_mut(wid)) {
                    match &mut win.backend {
                        Backend::Cpu { .. } => {
                            win.renderer.textures.remove(&tex);
                        }
                        #[cfg(feature = "gui-gpu")]
                        Backend::Gpu(gpu) => gpu.free_texture(tex),
                    }
                }
            }
            // A window's picture resampled off this thread (ADR-695): every window takes what
            // its worker finished, and one that took something repaints whole.
            UserEvent::PicturesLanded => {
                for win in self.wins.values_mut() {
                    if win.renderer.pictures.take_landed() {
                        win.renderer.invalidate();
                        #[cfg(feature = "gui-gpu")]
                        if let Backend::Gpu(gpu) = &mut win.backend {
                            gpu.pictures_landed();
                        }
                        win.window.request_redraw();
                    }
                }
            }
            UserEvent::InputMode { id, pixels } => {
                if let Some(win) = self.ids.get(&id).and_then(|wid| self.wins.get_mut(wid))
                    && win.pixel_input != pixels
                {
                    win.pixel_input = pixels;
                    publish_size(win);
                }
            }
            UserEvent::Sound { id, snd, data } => {
                if let Some(win) = self.ids.get(&id).and_then(|wid| self.wins.get_mut(wid)) {
                    win.sounds.insert(snd, data);
                }
            }
            UserEvent::SoundFree { id, snd } => {
                if let Some(win) = self.ids.get(&id).and_then(|wid| self.wins.get_mut(wid)) {
                    win.sounds.remove(&snd);
                }
            }
            UserEvent::Draw { id, ops } => {
                if let Some(w) = self.ids.get(&id).and_then(|wid| self.wins.get_mut(wid)) {
                    // Brood finished rendering; the next momentum step may fire.
                    w.scroll_pending = false;
                    // The first frame to arrive after an input is taken as the one that
                    // answers it (ADR-440): the mailbox between hides which turn of the
                    // app's loop the input caused, so this is the honest approximation.
                    let input_at = w.input_at.take();
                    // A frame identical to the one on screen — a timer that fired and
                    // changed nothing, a model turn whose view is unchanged — costs no
                    // repaint at all. The op vocabulary is plain data, so equality is exact.
                    // A window still hidden paints it anyway: it is the app's first frame,
                    // and the window waits for one to show itself.
                    if ops == w.frame && w.shown {
                        return;
                    }
                    // Two frames before one paint keep the EARLIER input's stamp: the
                    // paint answers both.
                    w.draw_input = w.draw_input.or(input_at);
                    // Painting before this frame arrived would present the bare background.
                    w.drawn = true;
                    // Sounds are ops of the frame: each `[:sound …]` plays once, here, as
                    // the frame arrives — not on repaints, and (by the equality above) not
                    // for a frame that repeats the last one verbatim.
                    for op in &ops {
                        if let Op::Sound { id: snd, vol } = op
                            && let Some(data) = w.sounds.get(snd)
                        {
                            crate::host::audio::play(data, *vol);
                        }
                    }
                    reconcile_music(w, &ops);
                    w.renderer.emitters.frame_arrived(&ops);
                    // The rows this frame lays out (ADR-610): its seam, and whether row 0 is
                    // a band. A change of band moves the top margin, so the grid holds
                    // another number of rows: the app is told, and lays out for it.
                    if w.renderer.set_layout(rows::frame_layout(&ops)) {
                        publish_size(w);
                    }
                    // Refresh the cursor hot-zones from this frame, then store it. The walk
                    // goes INTO the regions (`ScrollRegion`, `CellRegion`), resolving each
                    // zone to pixels with the metrics its own region is painted at.
                    w.zones = cursor_zones(&ops, &w.renderer);
                    w.frame = ops;
                    w.bleed.invalidate();
                    // A caret that moved, changed or appeared starts its blink over, shown; the
                    // same caret in a new frame (a spinner turned, a line elsewhere changed)
                    // keeps blinking where it was (ADR-469).
                    w.caret_blink = match (blinking_caret(&w.frame), w.caret_blink.take()) {
                        (None, _) => None,
                        (Some(caret), Some(clock)) if clock.caret == *caret => Some(clock),
                        (Some(caret), _) => CaretBlink::of(caret),
                    };
                    update_ime_area(w);
                    w.window.request_redraw();
                }
            }
            UserEvent::Settle { id } => {
                if let Some(w) = self.ids.get(&id).and_then(|wid| self.wins.get_mut(wid)) {
                    // As a Draw would: the app is done with the last scroll step, so the
                    // next momentum step may fire — and the input it was answering changed
                    // nothing, so no later frame is its answer.
                    w.scroll_pending = false;
                    w.input_at = None;
                }
            }
            UserEvent::Close { id } => {
                if let Some(wid) = self.ids.remove(&id) {
                    self.wins.remove(&wid); // dropping the window closes it
                }
            }
            // Un-minimise, then raise + focus, so a singleton window that's
            // already open is surfaced rather than re-spawned (behind gui-focus).
            UserEvent::Focus { id } => {
                if let Some(w) = self.ids.get(&id).and_then(|wid| self.wins.get(wid)) {
                    w.window.set_minimized(false);
                    w.window.focus_window();
                }
            }
            // Confine the pointer to the window (or release it). `Confined` keeps
            // it inside but still moving (so absolute mouse-look maps edge-to-edge);
            // some platforms only offer `Locked`, so fall back to that.
            UserEvent::Grab { id, on } => {
                if let Some(w) = self.ids.get(&id).and_then(|wid| self.wins.get(wid)) {
                    let mode = if on {
                        CursorGrabMode::Confined
                    } else {
                        CursorGrabMode::None
                    };
                    if on && w.window.set_cursor_grab(mode).is_err() {
                        let _ = w.window.set_cursor_grab(CursorGrabMode::Locked);
                    } else if !on {
                        let _ = w.window.set_cursor_grab(CursorGrabMode::None);
                    }
                }
            }
            // A thin client's daemon says its loop is busy, or not (ADR-491).
            UserEvent::Busy { id, on } => {
                if let Some(w) = self.ids.get(&id).and_then(|wid| self.wins.get_mut(wid)) {
                    w.busy_remote = on;
                    show_busy(w);
                }
            }
            // Maximise the window (fill the work area, keep the title bar) or
            // restore it. The resize that follows republishes the new cell grid
            // like any other, so the loop re-renders at the bigger size.
            UserEvent::Minimize { id } => {
                if let Some(w) = self.ids.get(&id).and_then(|wid| self.wins.get(wid)) {
                    w.window.set_minimized(true);
                }
            }
            // Both drags hand the gesture to the window manager, which then owns
            // it until the button is released — so a failure here (an unsupported
            // platform, or no button actually held) is ignored rather than raised:
            // the app asked politely and the WM declined.
            UserEvent::DragMove { id } => {
                if let Some(w) = self.ids.get(&id).and_then(|wid| self.wins.get(wid)) {
                    let _ = w.window.drag_window();
                }
            }
            UserEvent::DragResize { id, dir } => {
                if let Some(w) = self.ids.get(&id).and_then(|wid| self.wins.get(wid)) {
                    use winit::window::ResizeDirection as RD;
                    let d = match dir.as_str() {
                        "north" => RD::North,
                        "south" => RD::South,
                        "east" => RD::East,
                        "west" => RD::West,
                        "north-east" => RD::NorthEast,
                        "north-west" => RD::NorthWest,
                        "south-east" => RD::SouthEast,
                        _ => RD::SouthWest,
                    };
                    let _ = w.window.drag_resize_window(d);
                }
            }
            UserEvent::Maximize { id, on } => {
                if let Some(w) = self.ids.get(&id).and_then(|wid| self.wins.get(wid)) {
                    w.window.set_maximized(on);
                }
            }
            // Borderless-fullscreen (`Borderless(None)` = the current monitor) or
            // restore. Like Maximize the resize that follows re-renders the grid.
            UserEvent::Fullscreen { id, on } => {
                if let Some(w) = self.ids.get(&id).and_then(|wid| self.wins.get(wid)) {
                    w.window.set_fullscreen(if on {
                        Some(Fullscreen::Borderless(None))
                    } else {
                        None
                    });
                }
            }
            // Cell font. `id: Some(w)` retunes just that window, leaving the
            // global default alone (so two windows can differ). `id: None` is
            // the global default: remembered for future windows and applied to
            // every open one. Either way a target window recomputes its grid +
            // republishes its size + redraws (`apply_font`).
            UserEvent::Font { id, family, px } => match id {
                Some(target) => {
                    if let Some(w) = self.ids.get(&target).and_then(|wid| self.wins.get_mut(wid)) {
                        apply_font(w, family, px);
                    }
                }
                None => {
                    if let Some(f) = family {
                        self.defaults.family = Some(f);
                    }
                    if let Some(p) = px {
                        self.defaults.px = p.max(1.0);
                    }
                    for w in self.wins.values_mut() {
                        apply_font(w, family, px);
                    }
                }
            },
            UserEvent::Inset { px } => {
                self.defaults.inset = px.max(0.0);
                for w in self.wins.values_mut() {
                    w.renderer.set_inset(px);
                    update_cells(&w.window, &w.renderer, &w.size);
                    w.window.request_redraw();
                }
            }
            UserEvent::Background { rgb } => {
                self.defaults.bg = rgb;
                for w in self.wins.values_mut() {
                    w.renderer.set_bg(rgb);
                    // Background is a pure repaint — no metric change, so no
                    // `update_cells`/snap.
                    w.window.request_redraw();
                }
            }
            UserEvent::Foreground { rgb } => {
                self.defaults.fg = rgb;
                for w in self.wins.values_mut() {
                    w.renderer.set_fg(rgb);
                    w.window.request_redraw();
                }
            }
            UserEvent::LineHeight { mult } => {
                self.defaults.line_height = mult;
                for w in self.wins.values_mut() {
                    w.renderer.set_line_height(mult);
                    // The cell height moved, so the grid did: the same path as a font
                    // change — new (cols, rows) to the app, then a repaint.
                    publish_size(w);
                    w.window.request_redraw();
                }
            }
            UserEvent::TextAa { mode } => {
                self.defaults.text_aa = mode;
                for w in self.wins.values_mut() {
                    w.renderer.set_text_aa(mode);
                    w.window.request_redraw();
                }
            }
            UserEvent::TextContrast { gamma } => {
                self.defaults.text_contrast = gamma;
                for w in self.wins.values_mut() {
                    w.renderer.set_text_contrast(gamma);
                    w.window.request_redraw();
                }
            }
            // Register a font family from raw TTF bytes; parse here and share it
            // with every renderer. A bad font is dropped (the family stays
            // unregistered, so `:family` falls back to the default).
            UserEvent::RegisterFamily { name, faces, paths } => {
                self.families.borrow_mut().register(name, *faces, paths);
                fonts_changed(&mut self.wins);
            }
            // By installed name: resolved against the system font database (scanned on
            // the first such call), the bundled mono standing in for a name not found.
            UserEvent::RegisterInstalledFamily { name, family_name } => {
                self.families
                    .borrow_mut()
                    .register_installed(name, &family_name);
                fonts_changed(&mut self.wins);
            }
            UserEvent::FontInfo { family, reply } => {
                let family = family
                    .or(self.defaults.family)
                    .unwrap_or_else(|| value::intern(DEFAULT_FAMILY));
                let info = self.families.borrow().info(family, self.defaults.ligatures);
                let _ = reply.send(info);
            }
            UserEvent::Ligatures { on } => {
                self.defaults.ligatures = on;
                for w in self.wins.values_mut() {
                    w.renderer.set_ligatures(on);
                    w.window.request_redraw();
                }
            }
            // The window's frame as its paint draws it: the same bled frame, margins and
            // renderer (ADR-609). The raster forgets the retained frame, so the next paint
            // repaints whole rather than trusting a canvas the picture may have moved.
            UserEvent::Capture { id, reply } => {
                let answer = match self.ids.get(&id).and_then(|wid| self.wins.get_mut(wid)) {
                    Some(w) => {
                        let inner = w.window.inner_size();
                        let (width, height) = (inner.width.max(1), inner.height.max(1));
                        let margins =
                            bleed::Margins::of(&w.renderer, width as usize, height as usize);
                        let frame = w.bleed.frame(&w.frame, margins).to_vec();
                        let image = offscreen::raster(
                            &mut w.renderer,
                            &frame,
                            width as usize,
                            height as usize,
                        );
                        w.window.request_redraw();
                        Ok(image)
                    }
                    None => Err("gui window not open".to_string()),
                };
                let _ = reply.send(answer);
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(_gate) = enter_gate() else {
            return;
        };
        let Some(w) = self.wins.get_mut(&window_id) else {
            return;
        };
        // Stamp the input this event is (ADR-440): a key press, a button press, a wheel step.
        // The earliest unanswered one is kept; the next frame to arrive answers it, and a
        // turn that settles with nothing to draw (`settle`) drops it. A press the arms below
        // drop (an auto-repeat) is stamped too — the frame the app's own repeat draws
        // answers it, which is what the user waits on. A bare modifier is not: `C-x` is
        // timed from the `x`, not from however long Ctrl was held before it. Each stamp names
        // its kind, so key-to-paint is counted apart from the pointer (ADR-612).
        let input = match &event {
            WindowEvent::KeyboardInput {
                event: key_event,
                is_synthetic,
                ..
            } => (key_event.state == ElementState::Pressed
                && !is_synthetic
                && !is_modifier_key(key_event))
            .then_some(InputKind::Key),
            WindowEvent::MouseInput { state, .. } => {
                (*state == ElementState::Pressed).then_some(InputKind::Pointer)
            }
            // a horizontal-only wheel event scrolls nothing here, so no frame answers it
            WindowEvent::MouseWheel { delta, .. } => match delta {
                MouseScrollDelta::LineDelta(_, y) => *y != 0.0,
                MouseScrollDelta::PixelDelta(position) => position.y != 0.0,
            }
            .then_some(InputKind::Pointer),
            // an input method's commit is typing, whatever the keyboard did
            WindowEvent::Ime(winit::event::Ime::Commit(text)) => {
                (!text.is_empty()).then_some(InputKind::Key)
            }
            _ => None,
        };
        let is_input = input.is_some();
        if is_input && w.busy_check.is_none() {
            // the loop may now be busy with this input: look in a moment (ADR-491)
            w.busy_check = Some(Instant::now() + BUSY_PROBE);
        }
        if is_input {
            // the caret shows from the moment the user does something, whatever its blink
            // phase, and the next blink is a whole interval away (ADR-469)
            wake_caret(w, true);
            if w.input_at.is_none() {
                w.input_at = input.map(|kind| (Instant::now(), kind));
            }
        }
        // A key that reached the window ends a composition that already ended: one that
        // was cancelled (Escape sends an empty preedit and no commit) must not make the
        // next one-character commit read as composed text.
        if let WindowEvent::KeyboardInput {
            event: key_event, ..
        } = &event
            && key_event.state == ElementState::Pressed
            && w.composition == Composition::Ended
        {
            w.composition = Composition::Idle;
        }
        match event {
            // The window's close button → a dedicated `:close` message,
            // distinct from the Escape *key* (`:escape`): a frontend signal
            // ("the user wants this window gone"), not a keystroke. The Brood
            // loop tears down (calling gui-close) on its own terms — `ui-run`
            // quits on `:close` automatically; a raw loop matches it like any
            // other input. Keeping it separate means an app that binds Escape
            // to cancel/normal-mode can still be closed by the X button.
            WindowEvent::CloseRequested => {
                deliver(w.subscriber, Message::Keyword(value::intern("close")));
            }
            WindowEvent::ModifiersChanged(m) => {
                use winit::keyboard::ModifiersKeyState;
                w.mods = m.state();
                w.option_sides = (
                    m.lalt_state() == ModifiersKeyState::Pressed,
                    m.ralt_state() == ModifiersKeyState::Pressed,
                );
            }
            WindowEvent::Resized(_) => {
                // The sub-cell remainder is placed at paint time (`Renderer::rows`: on the
                // right, and at the frame's seam or under the last row, ADR-610), so there's
                // no window-resize snap to do here — it works on every WM. The caret's pixels
                // move with a seam's gap, so the input method is told again.
                update_ime_area(w);
                // Wake the app loop so it re-renders at the new (cols, rows)
                // now, rather than after its (possibly long) poll timeout.
                publish_size(w);
                w.window.request_redraw();
            }
            // We deliberately ignore 0.30's `inner_size_writer` (which could
            // request a specific new inner size): the cell grid *reflows* to
            // whatever size the window is, so we just re-derive the scale and
            // recompute (cols, rows) from the current `inner_size()`.
            WindowEvent::ScaleFactorChanged { .. } => {
                w.renderer.set_scale(w.window.scale_factor());
                publish_size(w);
                w.window.request_redraw();
            }
            WindowEvent::KeyboardInput {
                event: ke,
                is_synthetic,
                ..
            } => match ke.state {
                // Every press goes through `Keyboard::press`, which tells a repeat from a fresh
                // press by TRANSITION, not winit's `ke.repeat` flag: on
                // GNOME/Wayland that flag is unreliable — held keys arrive as a
                // flood of `repeat == false` presses (ADR-086). A press for the
                // key already held (no release between) is the repeat. A genuine
                // re-press (double-tap) comes only after a release, so it still
                // registers. Synthetic presses (winit replaying held keys on focus
                // *gain*) are dropped — they'd be phantom keystrokes.
                //
                // A repeat is dropped under `{:key-repeat :app}` (ADR-086: the app
                // repeats from `gui-held-key`) and, under `:os` (ADR-441), delivered as
                // another press through `RepeatGate`, so at most one is ever undelivered
                // and a slow frame cannot build a backlog that plays on after key-up. A
                // keystroke that typed several characters (a compose sequence) is
                // `[:text …]`, not a key, and is held as itself: its repeats are found
                // the same way, and `gui-held-key` reports it as `[:text …]`.
                ElementState::Pressed if !is_synthetic => {
                    let press = KeyPress::of(&ke);
                    w.non_latin_layout = layout_is_non_latin(&press, w.non_latin_layout);
                    let stroke =
                        translate_keystroke(&press, w.chord_modifiers(), w.non_latin_layout);
                    match stroke.and_then(|stroke| {
                        w.keyboard.press(stroke, ke.physical_key, Instant::now())
                    }) {
                        Some(KeyMessage::Press(message, receipt)) => {
                            deliver_credited(w.subscriber, message, receipt)
                        }
                        Some(KeyMessage::Repeat(message, credit)) => {
                            deliver_credited(w.subscriber, message, credit)
                        }
                        Some(KeyMessage::Restroke { up, press, receipt }) => {
                            deliver(w.subscriber, up);
                            deliver_credited(w.subscriber, press, receipt);
                        }
                        None => {}
                    }
                }
                ElementState::Pressed => {} // synthetic press (focus-gain replay)
                // Key release → `[:key-up <what its press delivered>]` (`Keyboard::release`,
                // which matches it to its press by PHYSICAL key, so a shifted chord whose
                // Shift came up first still names its press). Synthetic releases count too
                // (winit emits them for a key let go while unfocused — exactly when we must
                // stop).
                ElementState::Released => {
                    let translated =
                        translate_key(&KeyPress::of(&ke), w.chord_modifiers(), w.non_latin_layout);
                    if let Some(message) = w.keyboard.release(ke.physical_key, translated) {
                        deliver(w.subscriber, message);
                    }
                }
            },
            // Losing focus (Alt-Tab away mid-hold) is the case a key-up can go
            // missing entirely — the release happens in another window. Deliver
            // `:blur` so the app can drop any held key and stop repeating; a
            // belt-and-suspenders backstop beside the synthetic releases above
            // (ADR-086). Focus *gain* is `:focus-in`, below.
            WindowEvent::Focused(false) => {
                // A window without focus holds its caret still and shown (ADR-469).
                wake_caret(w, false);
                // Drop the held key: we can't observe its release while unfocused,
                // so `gui-held-key` must not keep reporting it (the poll-based stop)
                // and the `:blur` is the event-based stop. Both, belt-and-braces.
                w.keyboard.blur();
                deliver(w.subscriber, Message::Keyword(value::intern("blur")));
                // A composition does not survive the window losing focus on every
                // platform's IME, and not every one says so: clear what the app shows.
                end_composition(w);
                // Same for a held mouse button: its release may land off-window /
                // unfocused and never reach us, so synthesize one now (see CursorLeft).
                if let Some(b) = w.held.take() {
                    let (col, row) = w.cursor;
                    deliver(
                        w.subscriber,
                        mouse_message(&release_of(
                            b,
                            col,
                            row,
                            w.pointer_at(),
                            w.pointer_partial(),
                            &w.mods,
                        )),
                    );
                }
            }
            // Focus GAIN, as `:focus-in` — the pair of `:blur` (ADR-441). What an app
            // needs to repaint focus-dependent chrome (a hollow cursor turned solid), to
            // re-check files changed while it was away, and to know the held-key state it
            // dropped on `:blur` starts clean. Not `:focus`: that is `ui-run`'s
            // raise-this-window control message, which the loop acts on itself.
            WindowEvent::Focused(true) => {
                wake_caret(w, true);
                deliver(w.subscriber, Message::Keyword(value::intern("focus-in")));
            }
            // The input method (ADR-441). A composition in progress is `[:preedit text
            // range]` — shown at point by the app, not inserted — and an empty one clears
            // it; a commit is `[:text …]`, or a plain key when the IME merely passed one
            // character through (`commit_message`). A disabled IME drops whatever
            // composition was showing, so the app is told to clear it.
            WindowEvent::Ime(ime) => match ime {
                winit::event::Ime::Enabled => {
                    // The candidate window must be told where the caret is again.
                    w.ime_area = None;
                    update_ime_area(w);
                }
                winit::event::Ime::Preedit(text, cursor) => {
                    if text.is_empty() {
                        if w.composition != Composition::Showing {
                            return;
                        }
                        w.composition = Composition::Ended;
                    } else {
                        w.composition = Composition::Showing;
                    }
                    deliver(w.subscriber, preedit_message(&text, cursor));
                }
                winit::event::Ime::Commit(text) => {
                    // winit clears the preedit just before the commit that ends it, so a
                    // composition that just ended counts as composed too.
                    let composed = w.composition != Composition::Idle;
                    w.composition = Composition::Idle;
                    if let Some(message) = commit_message(&text, composed, w.chord_modifiers()) {
                        deliver(w.subscriber, message);
                    }
                }
                winit::event::Ime::Disabled => end_composition(w),
            },
            WindowEvent::CursorLeft { .. } => {
                // The pointer left the window. If a button was held, its release happens
                // outside and we never see it — so the NEXT re-entry's motion would emit
                // a phantom `:drag` and the app would think the button is still pressed.
                // Synthesize the release + clear `held` (mirrors the keyboard blur fix).
                if let Some(b) = w.held.take() {
                    let (col, row) = w.cursor;
                    deliver(
                        w.subscriber,
                        mouse_message(&release_of(
                            b,
                            col,
                            row,
                            w.pointer_at(),
                            w.pointer_partial(),
                            &w.mods,
                        )),
                    );
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                // Track the pointer cell. Bare motion (no button) isn't emitted —
                // no consumer, and a per-pixel event would flood + force redraws.
                // But while a button is held, crossing into a NEW cell emits a
                // `:drag` (cell-granular, so still bounded), which is how a divider
                // drag is tracked (ADR-077).
                let psz = w.window.inner_size();
                let cell = px_to_cell(
                    position,
                    &w.renderer,
                    psz.width as usize,
                    psz.height as usize,
                );
                let px = px_clamped(position);
                // the strip a frame paints its next line into is a cell of its own (ADR-615): the
                // seam's row number, but not the row bottom-anchored on it
                let partial = w
                    .renderer
                    .rows(psz.width as usize, psz.height as usize)
                    .in_strip(position.y);
                // A pixel-input window (a game) reports every pixel of motion — that IS
                // the grain it asked for, and winit already coalesces motion per frame —
                // where a cell-grid window reports only a cell crossing.
                let moved = if w.pixel_input {
                    px != w.cursor_px
                } else {
                    cell != w.cursor || partial != w.cursor_partial
                };
                w.cursor = cell;
                w.cursor_partial = partial;
                w.cursor_px = px;
                w.cursor_at = px_to_cell_frac(
                    position,
                    &w.renderer,
                    psz.width as usize,
                    psz.height as usize,
                );
                if moved {
                    let (col, row) = w.pointer();
                    // While a button is held this is a `:drag`; otherwise it's a
                    // bare `:move` (button nil). Either way it's cell-granular (only
                    // on crossing into a new cell), so it stays bounded — no per-pixel
                    // flood. Free `:move` is what lets an app do mouse-look / hover
                    // without requiring a click.
                    let action = if w.held.is_some() {
                        MouseAction::Drag
                    } else {
                        MouseAction::Move
                    };
                    deliver(
                        w.subscriber,
                        mouse_message(&Mouse {
                            action,
                            button: w.held,
                            row,
                            col,
                            ctrl: w.mods.control_key(),
                            alt: w.mods.alt_key(),
                            shift: w.mods.shift_key(),
                            count: 0,
                            scroll_dy: 0.0,
                            at: w.pointer_at(),
                            partial: w.pointer_partial(),
                        }),
                    );
                }
                // Hover cursor: show a zone's shape (e.g. a resize cursor on a divider,
                // a pointing hand on a link) while the pointer is over it. Locally
                // handled — no event reaches the app, so no redraw flood. (ADR-080.)
                //
                // Tested per PIXEL, not per cell: a zone inside a zoomed `CellRegion` is
                // a fraction of a window cell tall, so two neighbouring zones can live in
                // one cell — hit-testing only when the *cell* changed would step straight
                // over them. The `set_cursor` below is still guarded by a change in shape,
                // which is the call that costs anything.
                // The zones are in the grid's uniform pixels (`cursor_zones`); the pointer is
                // taken there through the row map's inverse, as a click is (ADR-610).
                let (width, height) = (psz.width as usize, psz.height as usize);
                let (grid_x, _) = w.renderer.grid_origin(width, height);
                let grid_y = w.renderer.rows(width, height).uniform_y(position.y);
                let want = shape_at(
                    &w.zones,
                    position.x as isize - grid_x as isize,
                    grid_y.floor() as isize,
                );
                if want != w.shape {
                    w.shape = want;
                    // the busy cursor wins while the loop is stuck; its end restores this one
                    if !w.busy_shown {
                        w.window
                            .set_cursor(want.map(cursor_icon).unwrap_or(CursorIcon::Default));
                    }
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button,
                ..
            } => {
                if let Some(b) = translate_button(button) {
                    w.held = Some(b);
                    let (col, row) = w.pointer();
                    // Click-chain count: a fresh press in the same cell, with the same
                    // button, within MULTI_CLICK_MS of the previous extends the chain
                    // (double/triple-click); anything else restarts at 1.
                    let now = Instant::now();
                    let count = match w.last_click {
                        Some((c, lb, lcell, lt))
                            if lb == b
                                && lcell == (col, row)
                                && now.duration_since(lt).as_millis() <= MULTI_CLICK_MS =>
                        {
                            c.saturating_add(1)
                        }
                        _ => 1,
                    };
                    w.last_click = Some((count, b, (col, row), now));
                    deliver(
                        w.subscriber,
                        mouse_message(&Mouse {
                            action: MouseAction::Press,
                            button: Some(b),
                            row,
                            col,
                            ctrl: w.mods.control_key(),
                            alt: w.mods.alt_key(),
                            shift: w.mods.shift_key(),
                            count,
                            scroll_dy: 0.0,
                            at: w.pointer_at(),
                            partial: w.pointer_partial(),
                        }),
                    );
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button,
                ..
            } => {
                if let Some(b) = translate_button(button) {
                    w.held = None;
                    let (col, row) = w.pointer();
                    deliver(
                        w.subscriber,
                        mouse_message(&Mouse {
                            action: MouseAction::Release,
                            button: Some(b),
                            row,
                            col,
                            ctrl: w.mods.control_key(),
                            alt: w.mods.alt_key(),
                            shift: w.mods.shift_key(),
                            count: 0,
                            scroll_dy: 0.0,
                            at: w.pointer_at(),
                            partial: w.pointer_partial(),
                        }),
                    );
                }
            }
            WindowEvent::MouseWheel { delta, phase, .. } => {
                // Positive y scrolls up (away from the user). LineDelta is in discrete
                // line units; PixelDelta (trackpad) is in physical pixels — normalised
                // here to line units so Brood gets a consistent float in both cases.
                //
                // A wheel notch (LineDelta) scrolls its lines at once or glides, per the
                // window's `WheelMode`; a trackpad (PixelDelta) tracks the finger and coasts.
                // Momentum itself is driven by `about_to_wait` with `WaitUntil(12ms)` so
                // it fires once per event-loop cycle after the queue is drained — no
                // flooding, no background thread.
                let ch = w.renderer.cell_h.max(1);
                match delta {
                    MouseScrollDelta::LineDelta(_, y) => {
                        // `{:wheel :glide}`: a notch is an IMPULSE into the kinetic scroll a
                        // trackpad flick runs, not a three-line jump: the glide reaches
                        // the notch's distance over ~half a second of fractional steps
                        // (`WHEEL_DECAY`), so the text moves rather than snaps. Notches in
                        // quick succession form a streak that scrolls slightly farther
                        // each — a spun wheel accelerates — capped so it never flings.
                        // A notch against the running direction stops the glide first.
                        let dy = y as f64;
                        if dy == 0.0 {
                            return;
                        }
                        // `{:wheel :lines}` (the default, ADR-441): the notch's whole
                        // distance at once, as a terminal and Emacs scroll — no glide, no
                        // streak. A notch the app has not drawn the last one of yet joins
                        // the backlog (`flush_wheel`), so a spun wheel is one message per
                        // frame carrying the sum, never a queue of them.
                        if w.wheel == WheelMode::Lines {
                            w.scroll_momentum_active = false;
                            w.scroll_velocity = 0.0;
                            if w.wheel_backlog * dy < 0.0 {
                                w.wheel_backlog = 0.0;
                            }
                            w.wheel_backlog += dy * WHEEL_LINES;
                            // what cannot go now, `about_to_wait` sends after the draw
                            flush_wheel(w, Instant::now());
                            return;
                        }
                        let now = Instant::now();
                        let streak = match w.wheel_last {
                            Some(t) if now.duration_since(t).as_millis() <= WHEEL_STREAK_MS => {
                                w.wheel_streak.saturating_add(1)
                            }
                            _ => 0,
                        };
                        w.wheel_last = Some(now);
                        w.wheel_streak = streak;
                        let same_way = w.scroll_momentum_active && w.scroll_velocity * dy > 0.0;
                        let carried = if same_way { w.scroll_velocity } else { 0.0 };
                        w.scroll_velocity = wheel_velocity(carried, streak, dy);
                        w.scroll_decay = WHEEL_DECAY;
                        w.scroll_pending = false;
                        w.scroll_momentum_active = true;
                        w.scroll_next_tick = now;
                        w.scroll_last_tick = now;
                        event_loop.set_control_flow(ControlFlow::WaitUntil(now));
                    }
                    MouseScrollDelta::PixelDelta(p) => {
                        let dy = p.y / ch as f64;
                        match phase {
                            TouchPhase::Cancelled => {
                                w.scroll_momentum_active = false;
                                w.scroll_pending = false;
                                w.scroll_velocity = 0.0;
                            }
                            TouchPhase::Ended => {
                                // Gesture lift-off. Apply the final delta (non-zero on
                                // some platforms), then let `about_to_wait` run the coast.
                                if dy != 0.0 {
                                    deliver_scroll(w, dy);
                                }
                                if w.scroll_velocity.abs() > 0.01 {
                                    let now = Instant::now();
                                    w.scroll_decay = TRACKPAD_DECAY;
                                    w.scroll_momentum_active = true;
                                    w.scroll_next_tick = now;
                                    w.scroll_last_tick = now;
                                } else {
                                    w.scroll_momentum_active = false;
                                    w.scroll_velocity = 0.0;
                                }
                            }
                            TouchPhase::Started => {
                                // Carry momentum only when the new gesture is in the
                                // same direction. Opposite-direction gestures reset so
                                // carried velocity can't briefly scroll the wrong way.
                                let carry =
                                    if w.scroll_momentum_active && w.scroll_velocity * dy > 0.0 {
                                        w.scroll_velocity
                                    } else {
                                        0.0
                                    };
                                w.scroll_momentum_active = false;
                                w.scroll_pending = false; // new gesture takes over; any in-flight event is superseded
                                w.scroll_velocity = carry + dy;
                                if dy != 0.0 {
                                    deliver_scroll(w, dy);
                                }
                            }
                            _ => {
                                // Moved: user has direct control; stop any running
                                // momentum and continue the EMA for this gesture.
                                w.scroll_momentum_active = false;
                                w.scroll_velocity = 0.8 * w.scroll_velocity + 0.2 * dy;
                                if dy != 0.0 {
                                    deliver_scroll(w, dy);
                                }
                            }
                        }
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                // Stall trace (BROOD_STALL_MS): the GUI paint runs on this native
                // thread, outside the green-process scheduler — a slow frame (glyph
                // rasterization, softbuffer present, vsync block) is invisible to the
                // "quantum" guard, so it gets its own guard here.
                // Nothing drawn yet and still hidden: painting now would present the bare
                // background, the flash on open. `about_to_wait` shows the window after
                // `SHOW_TIMEOUT` if the app never draws (ADR-440).
                if !w.drawn && !w.shown {
                    return;
                }
                let _sg = crate::core::heap::stall_guard("gui-paint");
                let paint_start = Instant::now();
                w.renderer.caret_hidden = w
                    .caret_blink
                    .as_ref()
                    .is_some_and(|clock| clock.hidden(paint_start));
                // The GPU paint's phases, or `None` from the CPU painter; nothing at all
                // when the GPU target skipped the frame (an occluded window) — nothing
                // reached the compositor, so the window stays hidden and the input
                // unanswered for the frame that does.
                let inner = w.window.inner_size();
                let margins =
                    bleed::Margins::of(&w.renderer, inner.width as usize, inner.height as usize);
                let frame = w.bleed.frame(&w.frame, margins);
                let painted: Option<Option<[u64; 5]>> = match &mut w.backend {
                    Backend::Cpu { surface, .. } => {
                        paint(surface, &w.window, &mut w.renderer, frame);
                        Some(None)
                    }
                    #[cfg(feature = "gui-gpu")]
                    Backend::Gpu(gpu) => gpu
                        .paint(frame, &mut w.renderer)
                        .map(|times| Some(times.phases())),
                };
                let presented = Instant::now();
                // The app has drawn: the window may appear. Shown even when the GPU skipped
                // this frame — a surface reports itself occluded while its window is hidden
                // (Metal does), so waiting for a presented frame would wait out
                // `SHOW_TIMEOUT` — and asked to paint again once visible.
                if !w.shown {
                    w.shown = true;
                    w.window.set_visible(true);
                    if painted.is_none() {
                        w.window.request_redraw();
                    }
                }
                if let Some(split) = painted
                    && let Some((input_at, kind)) = w.draw_input.take()
                {
                    record_latency(w, input_at, kind, paint_start, presented, split);
                }
                w.particles_next = w
                    .renderer
                    .emitters
                    .alive()
                    .then(|| Instant::now() + PARTICLE_FRAME);
            }
            _ => {}
        }
    }

    // `about_to_wait` fires after the event queue is fully drained and before
    // winit sleeps. This drives kinetic momentum. IMPORTANT: `about_to_wait` is
    // called on *every* event cycle, including each `UserEvent::Draw` that Brood
    // sends back after rendering a scroll step. Without a time guard this would
    // flood: deliver → Draw → about_to_wait → deliver → Draw → ∞. We gate each
    // delivery on `scroll_next_tick` (set to `now` when momentum starts, then
    // advanced by 12 ms after each step) so at most one step fires per 12 ms
    // regardless of how many times `about_to_wait` is called.
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let Some(_gate) = enter_gate() else {
            return;
        };
        let now = Instant::now();
        let mut next_wakeup: Option<Instant> = None;
        for w in self.wins.values_mut() {
            // A window whose app has not drawn within `SHOW_TIMEOUT` is shown anyway,
            // painted with what it has: never an invisible window (ADR-440).
            if !w.shown {
                let due = w.opened_at + SHOW_TIMEOUT;
                if now >= due {
                    w.shown = true;
                    w.window.set_visible(true);
                    w.window.request_redraw();
                } else {
                    next_wakeup = Some(next_wakeup.map_or(due, |t| t.min(due)));
                }
            }
            if let Some(due) = busy_cursor(w, now) {
                next_wakeup = Some(next_wakeup.map_or(due, |t| t.min(due)));
            }
            if let Some(due) = w.particles_next {
                if now >= due {
                    // the paint sets the next one while particles remain
                    w.particles_next = None;
                    w.window.request_redraw();
                } else {
                    next_wakeup = Some(next_wakeup.map_or(due, |t| t.min(due)));
                }
            }
            // The caret blinks here, not in the app's loop (ADR-469): a phase that differs
            // from the one painted asks for a paint — of the caret's row, on the CPU path —
            // and the window sleeps until the next toggle.
            if let Some(clock) = &w.caret_blink
                && w.shown
            {
                if clock.hidden(now) != w.renderer.caret_hidden {
                    w.window.request_redraw();
                }
                if let Some(due) = clock.next_toggle(now) {
                    next_wakeup = Some(next_wakeup.map_or(due, |t| t.min(due)));
                }
            }
            // A `{:wheel :lines}` backlog goes out once the app has drawn the last
            // scroll (or `WHEEL_HOLD` has passed); until then, wake to check again.
            if !flush_wheel(w, now) {
                let due = w.wheel_sent.map_or(now, |sent| sent + WHEEL_HOLD);
                next_wakeup = Some(next_wakeup.map_or(due, |t| t.min(due)));
            }
            if !w.scroll_momentum_active {
                continue;
            }
            // Skip until the minimum interval AND until Brood has finished
            // rendering the previous step. scroll_pending is cleared by the
            // UserEvent::Draw handler when Brood's pipeline completes.
            if now < w.scroll_next_tick || w.scroll_pending {
                next_wakeup = Some(match next_wakeup {
                    Some(t) => t.min(w.scroll_next_tick),
                    None => w.scroll_next_tick,
                });
                continue;
            }
            // Time-proportional decay: `scroll_decay` per 12 ms nominal (a trackpad's
            // long coast or a wheel notch's short glide), scaled by actual elapsed so
            // the curve is independent of render throughput.
            let elapsed_ms = now.duration_since(w.scroll_last_tick).as_secs_f64() * 1000.0;
            let decay = w.scroll_decay.powf((elapsed_ms / 12.0).clamp(0.5, 4.0));
            w.scroll_velocity *= decay;
            if w.scroll_velocity.abs() < 0.0005 {
                w.scroll_momentum_active = false;
                w.scroll_velocity = 0.0;
                continue;
            }
            w.scroll_last_tick = now;
            w.scroll_next_tick = now + Duration::from_millis(12);
            w.scroll_pending = true;
            deliver_scroll(w, w.scroll_velocity);
            next_wakeup = Some(match next_wakeup {
                Some(t) => t.min(w.scroll_next_tick),
                None => w.scroll_next_tick,
            });
        }
        event_loop.set_control_flow(match next_wakeup {
            Some(t) => ControlFlow::WaitUntil(t),
            None => ControlFlow::Wait,
        });
    }
}

/// The GUI thread body: build the one event loop, hand its proxy back to
/// `start_thread`, then run winit's loop forever — opening / closing / painting
/// windows from a registry as `UserEvent`s arrive. It never exits (winit can't
/// restart an event loop), so it idles harmlessly when no windows are open.
pub(super) fn run_gui(ready: Sender<Result<EventLoopProxy<UserEvent>, String>>) {
    // Two hosting modes, decided by `main_thread_hosting_required()`:
    //
    //   * Wayland/X11 — this runs on the dedicated `brood-gui` thread, and
    //     `with_any_thread(true)` is what permits a loop off the main thread.
    //   * everywhere else (macOS, Windows) — this runs ON the process main thread,
    //     because AppKit's run loop may live nowhere else and winit offers no escape
    //     hatch. `with_any_thread` does not exist on those platforms at all, which is
    //     why the call is gated rather than merely unnecessary (KI-125).
    #[allow(unused_mut)]
    let mut builder = EventLoop::<UserEvent>::with_user_event();
    #[cfg(any(
        target_os = "linux",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd"
    ))]
    builder.with_any_thread(true);
    let event_loop = match builder.build() {
        Ok(el) => el,
        Err(e) => {
            let _ = ready.send(Err(format!("event loop: {e}")));
            return;
        }
    };
    let _ = ready.send(Ok(event_loop.create_proxy()));
    // The X server dying would otherwise `exit(1)` the runtime from inside Xlib (ADR-631).
    install_x_io_error_handler();

    let mut app = GuiApp {
        wins: HashMap::new(),
        ids: HashMap::new(),
        families: default_families(),
        defaults: RenderDefaults::default(),
        resumed: false,
        pending_open: Vec::new(),
    };
    // `run_app` returns only when the loop cannot go on: nothing here asks it to exit, so
    // an error (a Wayland compositor gone, a protocol error) or a panic inside winit is the
    // display going away. Tell every window's app rather than leave it waiting for input.
    let ended = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        event_loop.run_app(&mut app)
    }));
    let why = match ended {
        Ok(Ok(())) => "the event loop ended".to_string(),
        Ok(Err(e)) => format!("the event loop failed: {e}"),
        Err(_) => "the event loop panicked".to_string(),
    };
    lose_display(&why);
}
