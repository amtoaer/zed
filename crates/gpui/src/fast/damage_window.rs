//! Damage accumulated until a frame is handed to the renderer.

use crate::{SceneDamage, Window};
pub(crate) fn present_skip_enabled() -> bool {
    if cfg!(feature = "bench") {
        return false;
    }
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("GPUI_EXPERIMENTAL_PRESENT_SKIP").map_or(true, |value| {
            value != "0"
                && !value.is_empty()
                && !value.eq_ignore_ascii_case("false")
                && !value.eq_ignore_ascii_case("off")
        })
    })
}
pub(crate) fn partial_render_enabled() -> bool {
    if cfg!(feature = "bench") {
        return false;
    }
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("GPUI_EXPERIMENTAL_PARTIAL_RENDER").map_or(true, |value| {
            value != "0"
                && !value.is_empty()
                && !value.eq_ignore_ascii_case("false")
                && !value.eq_ignore_ascii_case("off")
        })
    })
}

pub(crate) struct WindowDamage {
    pub(crate) pending: SceneDamage,
    presented: bool,
}
impl Default for WindowDamage {
    fn default() -> Self {
        Self {
            pending: SceneDamage::Full,
            presented: false,
        }
    }
}
pub(crate) fn compare(window: &Window) -> SceneDamage {
    if window.refreshing
        || crate::fast::composition::is_enabled(window)
        || !(present_skip_enabled() || partial_render_enabled())
    {
        SceneDamage::Full
    } else {
        SceneDamage::between(&window.rendered_frame.scene, &window.next_frame.scene)
    }
}
pub(crate) fn finish(window: &mut Window, damage: SceneDamage) {
    let pending = std::mem::replace(&mut window.fast_damage.pending, SceneDamage::Unchanged);
    window.fast_damage.pending = pending.union(damage);
    window.needs_present.set(
        !(present_skip_enabled()
            && window.fast_damage.presented
            && matches!(window.fast_damage.pending, SceneDamage::Unchanged)),
    );
}
pub(crate) fn present(window: &mut Window) {
    let damage = std::mem::replace(&mut window.fast_damage.pending, SceneDamage::Unchanged);
    if partial_render_enabled() {
        window
            .platform_window
            .draw_with_damage(&window.rendered_frame.scene, &damage);
    } else {
        window.platform_window.draw(&window.rendered_frame.scene);
    }
    window.fast_damage.presented = true;
}

pub(crate) fn draw_platform(window: &(impl crate::PlatformWindow + ?Sized), scene: &crate::Scene) {
    window.draw(scene);
}

#[cfg(test)]
mod tests {
    use crate::{
        AppContext, Context, IntoElement, ParentElement, Render, Styled, TestAppContext, Window,
        div, point, px,
    };
    #[gpui::test]
    fn test_damage_accumulates_until_present(cx: &mut TestAppContext) {
        struct DamageView(f32);

        impl Render for DamageView {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div().size_full().child(
                    div()
                        .absolute()
                        .left(px(self.0))
                        .top(px(20.))
                        .size(px(10.))
                        .bg(crate::rgb(0xffffff)),
                )
            }
        }

        cx.app.borrow_mut().mode = crate::app::GpuiMode::Production;
        for next_x in [60., 100.] {
            let handle = cx.add_window(|_, _| DamageView(20.));
            cx.update_window(handle.into(), |_, window, cx| {
                window.draw(cx).clear(cx);
                super::present(window);
            })
            .unwrap();
            handle
                .update(cx, |view, _, cx| {
                    view.0 = 60.;
                    cx.notify();
                })
                .unwrap();
            cx.update_window(handle.into(), |_, window, cx| {
                window.draw(cx).clear(cx);
            })
            .unwrap();
            handle
                .update(cx, |view, _, cx| {
                    view.0 = next_x;
                    cx.notify();
                })
                .unwrap();
            cx.update_window(handle.into(), |_, window, cx| {
                window.draw(cx).clear(cx);
                assert!(
                    window.needs_present.get(),
                    "unpresented changes must not be skipped"
                );
                for x in [25., 65., next_x + 5.] {
                    assert!(
                        window
                            .fast_damage
                            .pending
                            .contains_point(&point(px(x), px(25.)).scale(window.scale_factor())),
                        "earlier damage was lost: {:?}",
                        window.fast_damage.pending
                    );
                }
                super::present(window);
                assert!(matches!(
                    window.fast_damage.pending,
                    crate::SceneDamage::Unchanged
                ));
                window.draw(cx).clear(cx);
                assert!(
                    !window.needs_present.get(),
                    "an unchanged presented scene should still be skipped"
                );
            })
            .unwrap();
        }
    }
}

pub(crate) fn present_if_needed(window: &mut Window, required: bool) {
    if window.needs_present.get() || required {
        window.present();
    }
}

#[cfg(test)]
pub(crate) fn pending_presentation_wakes_a_parked_render_loop(cx: &mut crate::TestAppContext) {
    use crate::{AppContext, Empty};
    let window = cx.add_window(|_, _| Empty);
    let test_window = cx.test_window(window.into());

    assert!(test_window.simulate_scheduled_frame());
    assert!(test_window.simulate_scheduled_frame());
    assert!(!test_window.frame_scheduled());

    cx.update_window(window.into(), |_, window, cx| {
        window.refreshing = true;
        window.draw(cx).clear(cx)
    })
    .unwrap();

    assert!(
        test_window.frame_scheduled(),
        "a rendered scene awaiting presentation must wake the render loop"
    );
}
