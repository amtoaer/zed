#[cfg(feature = "wayland")]
pub(crate) fn wayland(
    window: &crate::linux::wayland::window::WaylandWindow,
    scene: &gpui::Scene,
    damage: Option<&gpui::SceneDamage>,
) {
    use crate::linux::wayland::window::{FrameLoop, PresentationState, RawWindow};
    use wayland_client::Proxy;

    let mut state = window.borrow_mut();
    let state = &mut *state;
    let Some(renderer) = &mut state.renderer else {
        return;
    };

    if renderer.device_lost() {
        let raw_window = RawWindow {
            window: state.surface.id().as_ptr().cast::<std::ffi::c_void>(),
            display: state
                .surface
                .backend()
                .upgrade()
                .unwrap()
                .display_ptr()
                .cast::<std::ffi::c_void>(),
        };
        match renderer.recover(&raw_window) {
            Ok(()) => {
                crate::fast::composition::wayland::Composition::renderers_lost(
                    &state.fast_composition,
                );
            }
            Err(err) => {
                log::warn!("GPU recovery failed, will retry on next frame: {err}");
            }
        }

        state.redraw_requested = true;
        return;
    }

    // Surface state changed during this GPUI tick is included in this presentation.
    state.redraw_requested = false;
    if state.pending_frame_callback.is_none() {
        let callback = state.surface.frame(&state.globals.qh, state.surface.id());
        state.pending_frame_callback = Some(callback);
    }
    if match damage {
        Some(damage) => renderer.draw_with_damage(scene, damage),
        None => renderer.draw(scene),
    } {
        state.presentation = PresentationState::Presented;
        window.0.frame_loop.set(FrameLoop::AwaitingCallback);
    } else {
        state.presentation = state.presentation.failed();
        window.0.frame_loop.set(FrameLoop::PresentationFailed);
    }

    if renderer.needs_redraw() {
        state.redraw_requested = true;
    }
}

#[cfg(feature = "x11")]
pub(crate) fn x11(
    window: &crate::linux::x11::window::X11Window,
    scene: &gpui::Scene,
    damage: Option<&gpui::SceneDamage>,
) {
    use crate::linux::x11::window::RawWindow;

    let mut inner = window.0.state.borrow_mut();
    let inner = &mut *inner;
    let Some(renderer) = &mut inner.renderer else {
        return;
    };

    if renderer.device_lost() {
        let raw_window = RawWindow {
            connection: as_raw_xcb_connection::AsRawXcbConnection::as_raw_xcb_connection(
                &*window.0.xcb,
            ) as *mut _,
            screen_id: inner.x_screen_index,
            window_id: window.0.x_window,
            visual_id: inner.visual_id,
        };
        match renderer.recover(&raw_window) {
            Ok(()) => {}
            Err(err) => {
                log::warn!("GPU recovery failed, will retry on next frame: {err}");
            }
        }

        inner.force_render_after_recovery = true;
        return;
    }

    match damage {
        Some(damage) => renderer.draw_with_damage(scene, damage),
        None => renderer.draw(scene),
    };

    if renderer.needs_redraw() {
        inner.force_render_after_recovery = true;
    }
}
