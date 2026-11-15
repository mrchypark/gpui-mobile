// Modified by Longbridge for gpui-fast.
use crate::{
    App, Bounds, ClipboardItem, Context, Entity, InputHandler, Pixels, TextInputConfiguration,
    UTF16Selection, Window,
};
use std::ops::Range;

/// Implement this trait to allow views to handle textual input when implementing an editor, field, etc.
///
/// Once your view implements this trait, you can use it to construct an [`ElementInputHandler<V>`].
/// This input handler can then be assigned during paint by calling [`Window::handle_input`].
///
/// Dismissing the software keyboard can leave the component focused. Call
/// [`Window::request_virtual_keyboard`] from an editing gesture, such as tapping
/// the field, to reopen it; repainting alone should not undo a dismissal.
///
/// After application-driven edits or selection changes, also call
/// [`Window::notify_text_input_changed`] or [`Window::notify_text_selection_changed`]
/// so native text services can invalidate their state.
///
/// UIKit text input also needs `text_length_utf16` (or a bounded
/// `text_input_editable_range`) and native selection updates through
/// `set_selected_text_range`. Supply `selection_bounds_for_range` for native
/// selection rectangles; it should return line/run fragments in one batch.
///
/// See [`InputHandler`] for details on how to implement each method.
pub trait EntityInputHandler: 'static + Sized {
    /// See [`InputHandler::text_for_range`] for details
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<String>;

    /// See [`InputHandler::selected_text_range`] for details
    fn selected_text_range(
        &mut self,
        ignore_disabled_input: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<UTF16Selection>;

    /// See [`InputHandler::marked_text_range`] for details
    fn marked_text_range(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Range<usize>>;

    /// See [`InputHandler::unmark_text`] for details
    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>);

    /// See [`InputHandler::paste`] for details
    fn paste(&mut self, item: ClipboardItem, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = item.text() {
            self.replace_text_in_range(None, &text, window, cx);
        }
    }

    /// See [`InputHandler::replace_text_in_range`] for details
    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    );

    /// See [`InputHandler::replace_and_mark_text_in_range`] for details
    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    );

    /// See [`InputHandler::bounds_for_range`] for details
    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>>;

    /// See [`InputHandler::selection_bounds_for_range`] for details.
    fn selection_bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        _element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Vec<Bounds<Pixels>> {
        Vec::new()
    }

    /// See [`InputHandler::character_index_for_point`] for details
    fn character_index_for_point(
        &mut self,
        point: crate::Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<usize>;

    /// See [`InputHandler::set_selected_text_range`] for details
    fn set_selected_text_range(
        &mut self,
        _range_utf16: Range<usize>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }

    /// See [`InputHandler::text_length_utf16`] for details
    fn text_length_utf16(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }

    /// See [`InputHandler::accepts_text_input`] for details
    fn accepts_text_input(&self, _window: &mut Window, _cx: &mut Context<Self>) -> bool {
        true
    }

    /// See [`InputHandler::text_input_configuration`] for details
    fn text_input_configuration(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> TextInputConfiguration {
        TextInputConfiguration::default()
    }

    /// See [`InputHandler::text_input_editable_range`] for details
    fn text_input_editable_range(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        None
    }
}

/// The canonical implementation of [`crate::PlatformInputHandler`]. Call [`Window::handle_input`]
/// with an instance during your element's paint.
pub struct ElementInputHandler<V> {
    view: Entity<V>,
    element_bounds: Bounds<Pixels>,
}

impl<V: 'static> ElementInputHandler<V> {
    /// Used in [`Element::paint`][element_paint] with the element's bounds, a `Window`, and a `App` context.
    ///
    /// [element_paint]: crate::Element::paint
    pub fn new(element_bounds: Bounds<Pixels>, view: Entity<V>) -> Self {
        ElementInputHandler {
            view,
            element_bounds,
        }
    }
}

impl<V: EntityInputHandler> InputHandler for ElementInputHandler<V> {
    fn selected_text_range(
        &mut self,
        ignore_disabled_input: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<UTF16Selection> {
        crate::fast::dependencies::query(&self.view, cx, |view, cx| {
            view.selected_text_range(ignore_disabled_input, window, cx)
        })
    }

    fn marked_text_range(&mut self, window: &mut Window, cx: &mut App) -> Option<Range<usize>> {
        crate::fast::dependencies::query(&self.view, cx, |view, cx| {
            view.marked_text_range(window, cx)
        })
    }

    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<String> {
        crate::fast::dependencies::query(&self.view, cx, |view, cx| {
            view.text_for_range(range_utf16, adjusted_range, window, cx)
        })
    }

    fn replace_text_in_range(
        &mut self,
        replacement_range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.view.update(cx, |view, cx| {
            view.replace_text_in_range(replacement_range, text, window, cx)
        });
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.view.update(cx, |view, cx| {
            view.replace_and_mark_text_in_range(
                range_utf16,
                new_text,
                new_selected_range,
                window,
                cx,
            )
        });
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut App) {
        self.view
            .update(cx, |view, cx| view.unmark_text(window, cx));
    }

    fn paste(&mut self, item: ClipboardItem, window: &mut Window, cx: &mut App) {
        self.view
            .update(cx, |view, cx| view.paste(item, window, cx));
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Bounds<Pixels>> {
        crate::fast::dependencies::query(&self.view, cx, |view, cx| {
            view.bounds_for_range(range_utf16, self.element_bounds, window, cx)
        })
    }

    fn selection_bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        window: &mut Window,
        cx: &mut App,
    ) -> Vec<Bounds<Pixels>> {
        self.view.update(cx, |view, cx| {
            view.selection_bounds_for_range(range_utf16, self.element_bounds, window, cx)
        })
    }

    fn character_index_for_point(
        &mut self,
        point: crate::Point<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<usize> {
        crate::fast::dependencies::query(&self.view, cx, |view, cx| {
            view.character_index_for_point(point, window, cx)
        })
    }

    fn set_selected_text_range(
        &mut self,
        range_utf16: Range<usize>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.view.update(cx, |view, cx| {
            view.set_selected_text_range(range_utf16, window, cx)
        })
    }

    fn element_bounds(&mut self, _window: &mut Window, _cx: &mut App) -> Option<Bounds<Pixels>> {
        Some(self.element_bounds)
    }

    fn text_length_utf16(&mut self, window: &mut Window, cx: &mut App) -> Option<usize> {
        crate::fast::dependencies::query(&self.view, cx, |view, cx| {
            view.text_length_utf16(window, cx)
        })
    }

    fn accepts_text_input(&mut self, window: &mut Window, cx: &mut App) -> bool {
        crate::fast::dependencies::query(&self.view, cx, |view, cx| {
            view.accepts_text_input(window, cx)
        })
    }

    fn prefers_ime_for_printable_keys(&mut self, window: &mut Window, cx: &mut App) -> bool {
        crate::fast::dependencies::query(&self.view, cx, |view, cx| {
            view.accepts_text_input(window, cx)
        })
    }

    fn text_input_configuration(
        &mut self,
        window: &mut Window,
        cx: &mut App,
    ) -> TextInputConfiguration {
        crate::fast::dependencies::query(&self.view, cx, |view, cx| {
            view.text_input_configuration(window, cx)
        })
    }

    fn text_input_editable_range(
        &mut self,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Range<usize>> {
        crate::fast::dependencies::query(&self.view, cx, |view, cx| {
            view.text_input_editable_range(window, cx)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AnyWindowHandle, AppContext as _, FocusHandle, InteractiveElement as _, IntoElement,
        ParentElement as _, Render, Styled as _, TestAppContext, TextInputAction,
        TextInputStateChange, canvas, div,
    };

    #[gpui::test]
    fn text_input_configuration_and_focus_state_are_forwarded_on_change(cx: &mut TestAppContext) {
        let custom = TextInputConfiguration {
            autocorrect: true,
            input_action: TextInputAction::Send,
            ..Default::default()
        };
        let window = cx.add_window({
            let custom = custom.clone();
            move |_, cx| ConfigurationTestView {
                focus_handle: cx.focus_handle(),
                configuration: custom,
                accepts_text_input: true,
                selection: None,
                marked: None,
                inset: crate::px(0.),
            }
        });
        let view = window.root(cx).unwrap();
        let test_window = cx.test_window(window.into());
        let window = AnyWindowHandle::from(window);
        let draw = |cx: &mut TestAppContext| {
            cx.update_window(window, |_, window, cx| window.draw(cx).clear(cx))
                .unwrap();
        };

        // Nothing is focused, so the platform learns the default configuration.
        draw(cx);
        assert_eq!(
            test_window.text_input_configurations(),
            vec![TextInputConfiguration::default()]
        );
        assert!(test_window.text_input_state_changes().is_empty());

        // Focusing the view routes its configuration to the platform.
        cx.update_window(window, |_, window, cx| {
            let focus_handle = view.read(cx).focus_handle.clone();
            window.focus(&focus_handle, cx);
        })
        .unwrap();
        draw(cx);
        assert_eq!(
            test_window.text_input_configurations(),
            vec![TextInputConfiguration::default(), custom.clone()]
        );
        assert_eq!(
            test_window.text_input_state_changes(),
            vec![TextInputStateChange::FocusGained]
        );

        // Redrawing without a change forwards nothing.
        draw(cx);
        assert_eq!(test_window.text_input_configurations().len(), 2);
        assert_eq!(test_window.text_input_state_changes().len(), 1);

        // Changing the configuration forwards the new value.
        let updated = TextInputConfiguration {
            suggestions: true,
            ..custom
        };
        view.update(cx, {
            let updated = updated.clone();
            |view, cx| {
                view.configuration = updated;
                cx.notify();
            }
        });
        draw(cx);
        assert_eq!(
            test_window.text_input_configurations().last(),
            Some(&updated)
        );
        assert_eq!(test_window.text_input_configurations().len(), 3);
        assert_eq!(test_window.text_input_state_changes().len(), 1);

        // Losing focus reverts the platform to the default configuration.
        cx.update_window(window, |_, window, cx| window.blur(cx))
            .unwrap();
        draw(cx);
        assert_eq!(
            test_window.text_input_configurations().last(),
            Some(&TextInputConfiguration::default())
        );
        assert_eq!(test_window.text_input_configurations().len(), 4);
        assert_eq!(
            test_window.text_input_state_changes(),
            vec![
                TextInputStateChange::FocusGained,
                TextInputStateChange::FocusLost
            ]
        );
    }

    #[gpui::test]
    fn focused_geometry_tracks_painted_handler_and_focus(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, cx| ConfigurationTestView {
            focus_handle: cx.focus_handle(),
            configuration: TextInputConfiguration::default(),
            accepts_text_input: true,
            selection: None,
            marked: None,
            inset: crate::px(0.),
        });
        let view = window.root(cx).unwrap();
        cx.update_window(window.into(), |_, window, cx| {
            assert!(window.focused_text_input_geometry().is_none());
            let focus = view.read(cx).focus_handle.clone();
            window.focus(&focus, cx);
            window.draw(cx).clear(cx);
            let (element, caret) = window.focused_text_input_geometry().unwrap();
            let painted = element.unwrap();
            assert!(caret.is_none());
            view.update(cx, |view, cx| {
                view.inset = crate::px(40.);
                cx.notify();
            });
            window.refresh();
            assert!(
                window.focused_text_input_geometry().is_none(),
                "geometry must not be used while invalidated layout awaits paint"
            );
            let rejected = std::rc::Rc::new(std::cell::Cell::new(false));
            let observed = rejected.clone();
            window.on_next_frame(move |window, _| {
                observed.set(window.focused_text_input_geometry().is_none());
            });
            window.simulate_next_frame(cx);
            assert!(rejected.get(), "a pre-draw callback must not see stale geometry");
            window.draw(cx).clear(cx);
            let repainted = window.focused_text_input_geometry().unwrap().0.unwrap();
            assert_eq!(repainted.left(), painted.left() + crate::px(40.));
            let other = cx.focus_handle();
            window.focus(&other, cx);
            assert!(window.focused_text_input_geometry().is_none());
            window.focus(&focus, cx);
            assert!(
                window.focused_text_input_geometry().is_none(),
                "a previous focus generation must not restore stale geometry"
            );
            window.draw(cx).clear(cx);
            assert!(window.focused_text_input_geometry().is_some());
            view.update(cx, |view, cx| {
                view.accepts_text_input = false;
                cx.notify();
            });
            window.draw(cx).clear(cx);
            assert!(window.focused_text_input_geometry().is_none());
        })
        .unwrap();
    }

    #[gpui::test]
    fn focused_geometry_uses_the_active_selection_endpoint(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, cx| ConfigurationTestView {
            focus_handle: cx.focus_handle(),
            configuration: TextInputConfiguration::default(),
            accepts_text_input: true,
            selection: Some((2..7, false)),
            marked: Some(1..9),
            inset: crate::px(0.),
        });
        let view = window.root(cx).unwrap();
        cx.update_window(window.into(), |_, window, cx| {
            window.focus(&view.read(cx).focus_handle.clone(), cx);
            window.draw(cx).clear(cx);
            let (_, caret) = window.focused_text_input_geometry().unwrap();
            assert_eq!(caret.unwrap().origin.x, crate::px(7.));
            view.update(cx, |view, cx| {
                view.selection = Some((2..7, true));
                cx.notify();
            });
            window.draw(cx).clear(cx);
            let (_, caret) = window.focused_text_input_geometry().unwrap();
            assert_eq!(caret.unwrap().origin.x, crate::px(2.));
        })
        .unwrap();
    }

    #[gpui::test]
    fn focused_geometry_rejects_changed_visible_bounds_without_callback(cx: &mut TestAppContext) {
        use crate::PlatformWindow as _;

        let window = cx.add_window(|_, cx| ConfigurationTestView {
            focus_handle: cx.focus_handle(),
            configuration: TextInputConfiguration::default(),
            accepts_text_input: true,
            selection: None,
            marked: None,
            inset: crate::px(0.),
        });
        let view = window.root(cx).unwrap();
        let mut platform_window = cx.test_window(window.into());
        platform_window.simulate_resize(crate::size(crate::px(400.), crate::px(800.)));
        cx.update_window(window.into(), |_, window, cx| {
            window.focus(&view.read(cx).focus_handle.clone(), cx);
            window.draw(cx).clear(cx);
            assert!(window.focused_text_input_geometry().is_some());
        })
        .unwrap();

        // Some backends expose updated insets without delivering their callback.
        platform_window.on_insets_changed(Box::new(|_| {}));
        platform_window.simulate_insets_change(crate::WindowInsets {
            ime: crate::Edges {
                bottom: crate::px(300.),
                ..Default::default()
            },
            ..Default::default()
        });
        cx.update_window(window.into(), |_, window, cx| {
            assert!(window.focused_text_input_geometry().is_none());
            window.draw(cx).clear(cx);
            assert!(window.focused_text_input_geometry().is_some());
        })
        .unwrap();
    }

    #[gpui::test]
    fn focused_geometry_after_next_paint_runs_after_draw_and_can_retry(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, cx| ConfigurationTestView {
            focus_handle: cx.focus_handle(),
            configuration: TextInputConfiguration::default(),
            accepts_text_input: true,
            selection: None,
            marked: None,
            inset: crate::px(0.),
        });
        let view = window.root(cx).unwrap();
        let calls = std::rc::Rc::new(std::cell::Cell::new(0));
        let old_left = cx.update_window(window.into(), |_, window, cx| {
            window.focus(&view.read(cx).focus_handle.clone(), cx);
            window.draw(cx).clear(cx);
            let old_left = window.focused_text_input_geometry().unwrap().0.unwrap().left();
            view.update(cx, |view, cx| {
                view.inset = crate::px(40.);
                cx.notify();
            });
            old_left
        })
        .unwrap();
        cx.update_window(window.into(), |_, window, cx| {
            let callback_calls = calls.clone();
            window.after_next_paint(move |window, _| {
                let left = window.focused_text_input_geometry().unwrap().0.unwrap().left();
                assert_eq!(left, old_left + crate::px(40.));
                callback_calls.set(callback_calls.get() + 1);
                let calls = callback_calls.clone();
                window.after_next_paint(move |_, _| calls.set(calls.get() + 1));
            });
            assert_eq!(calls.get(), 0);
            window.simulate_next_frame(cx);
            assert_eq!(calls.get(), 0, "the pre-draw frame callback is too early");
            window.draw(cx).clear(cx);
            assert_eq!(calls.get(), 0, "callback must wait for the window update to end");
        })
        .unwrap();
        assert_eq!(calls.get(), 1);
        cx.update_window(window.into(), |_, window, cx| {
            window.simulate_next_frame(cx);
            window.draw(cx).clear(cx);
            assert_eq!(calls.get(), 1);
        })
        .unwrap();
        assert_eq!(calls.get(), 2, "a retry must run after a later draw");
    }

    #[gpui::test]
    fn focused_geometry_after_paint_waits_for_chained_effects(cx: &mut TestAppContext) {
        use std::{cell::{Cell, RefCell}, rc::Rc};

        struct ChainedPaintView {
            input: Entity<ConfigurationTestView>,
            armed: Rc<Cell<bool>>,
            events: Rc<RefCell<Vec<&'static str>>>,
        }

        impl Render for ChainedPaintView {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let input = self.input.clone();
                let armed = self.armed.clone();
                let events = self.events.clone();
                div().size_full().child(self.input.clone()).child(
                    canvas(|_, _, _| {}, move |_, _, _, cx| {
                        if armed.replace(false) {
                            let input = input.clone();
                            let events = events.clone();
                            cx.defer(move |cx| {
                                events.borrow_mut().push("effect-a");
                                cx.defer(move |cx| {
                                    events.borrow_mut().push("effect-b");
                                    input.update(cx, |input, cx| {
                                        input.inset = crate::px(40.);
                                        cx.notify();
                                    });
                                });
                            });
                        }
                    }).absolute().size_full(),
                )
            }
        }

        let armed = Rc::new(Cell::new(false));
        let events = Rc::new(RefCell::new(Vec::new()));
        let window = cx.add_window({
            let armed = armed.clone();
            let events = events.clone();
            move |_, cx| ChainedPaintView {
                input: cx.new(|cx| ConfigurationTestView {
                    focus_handle: cx.focus_handle(),
                    configuration: TextInputConfiguration::default(),
                    accepts_text_input: true,
                    selection: None,
                    marked: None,
                    inset: crate::px(0.),
                }),
                armed,
                events,
            }
        });
        let view = window.root(cx).unwrap();
        let platform_window = cx.test_window(window.into());
        platform_window.simulate_active_status_change(true);
        let old_left = cx.update_window(window.into(), |_, window, cx| {
            let input = view.read(cx).input.clone();
            window.focus(&input.read(cx).focus_handle.clone(), cx);
            window.draw(cx).clear(cx);
            window.focused_text_input_geometry().unwrap().0.unwrap().left()
        }).unwrap();
        let observed = Rc::new(RefCell::new(None));
        cx.update_window(window.into(), |_, window, _| {
            let observed = observed.clone();
            let events = events.clone();
            window.after_next_paint(move |window, _| {
                events.borrow_mut().push("callback");
                *observed.borrow_mut() = Some(
                    window.focused_text_input_geometry().and_then(|(element, _)| element.map(|b| b.left()))
                );
            });
        }).unwrap();
        armed.set(true);
        platform_window.simulate_frame_request(crate::RequestFrameOptions {
            require_presentation: true,
            ..Default::default()
        });
        assert_eq!(&*events.borrow(), &["effect-a", "effect-b", "callback"]);
        let result = observed.borrow().expect("post-paint callback did not run");
        if let Some(left) = result {
            assert_eq!(left, old_left + crate::px(40.));
        } else {
            let retried = observed.clone();
            cx.update_window(window.into(), |_, window, _| {
                window.after_next_paint(move |window, _| {
                    *retried.borrow_mut() = Some(window.focused_text_input_geometry()
                        .and_then(|(element, _)| element.map(|b| b.left())));
                });
            }).unwrap();
            platform_window.simulate_frame_request(crate::RequestFrameOptions {
                require_presentation: true,
                ..Default::default()
            });
            assert_eq!(*observed.borrow(), Some(Some(old_left + crate::px(40.))));
        }
    }

    #[gpui::test]
    fn text_notifications_require_a_focused_input_and_track_field_changes(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, cx| ConfigurationTestView {
            focus_handle: cx.focus_handle(),
            configuration: TextInputConfiguration::default(),
            accepts_text_input: true,
            selection: None,
            marked: None,
            inset: crate::px(0.),
        });
        let view = window.root(cx).unwrap();
        let mut test_window = cx.test_window(window.into());
        let window = AnyWindowHandle::from(window);
        let first_focus = view.read_with(cx, |view, _| view.focus_handle.clone());
        cx.update_window(window, |_, window, cx| {
            window.focus(&first_focus, cx);
            window.draw(cx).clear(cx);
            window.notify_text_input_changed(&first_focus);
            window.notify_text_selection_changed(&first_focus);
        })
        .unwrap();

        use crate::PlatformWindow as _;
        let mut installed_handler = test_window.take_input_handler().unwrap();
        assert!(installed_handler.query_accepts_focused_text_input());
        assert_eq!(installed_handler.focus_id(), Some(first_focus.id));
        let second_focus = cx
            .update_window(window, |_, window, cx| {
                let second_focus = view.update(cx, |view, cx| {
                    view.focus_handle = cx.focus_handle();
                    cx.notify();
                    view.focus_handle.clone()
                });
                window.focus(&second_focus, cx);
                second_focus
            })
            .unwrap();
        assert!(!installed_handler.query_accepts_focused_text_input());
        test_window.set_input_handler(installed_handler);
        cx.update_window(window, |_, window, cx| {
            window.draw(cx).clear(cx);
            window.notify_text_input_changed(&first_focus);
        })
        .unwrap();
        assert_eq!(
            test_window.text_input_state_changes(),
            [
                TextInputStateChange::FocusGained,
                TextInputStateChange::ContentChanged,
                TextInputStateChange::SelectionChanged,
            ]
        );

        let installed_handler = test_window.take_input_handler().unwrap();
        assert_eq!(installed_handler.focus_id(), Some(second_focus.id));
        test_window.set_input_handler(installed_handler);
        cx.update_window(window, |_, window, cx| {
            window.draw(cx).clear(cx);
            window.notify_text_selection_changed(&second_focus);
        })
        .unwrap();

        view.update(cx, |view, cx| {
            view.accepts_text_input = false;
            cx.notify();
        });
        cx.update_window(window, |_, window, cx| {
            window.draw(cx).clear(cx);
            window.notify_text_input_changed(&second_focus);
            window.notify_text_selection_changed(&second_focus);
            let ordinary_focus = cx.focus_handle();
            window.focus(&ordinary_focus, cx);
            window.draw(cx).clear(cx);
            window.notify_text_input_changed(&ordinary_focus);
        })
        .unwrap();
        assert_eq!(
            test_window.text_input_state_changes(),
            [
                TextInputStateChange::FocusGained,
                TextInputStateChange::ContentChanged,
                TextInputStateChange::SelectionChanged,
                TextInputStateChange::SelectionChanged,
                TextInputStateChange::FocusLost,
            ]
        );
    }

    struct ConfigurationTestView {
        focus_handle: FocusHandle,
        configuration: TextInputConfiguration,
        accepts_text_input: bool,
        selection: Option<(std::ops::Range<usize>, bool)>,
        marked: Option<std::ops::Range<usize>>,
        inset: Pixels,
    }

    impl Render for ConfigurationTestView {
        fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let view = cx.entity();
            let focus_handle = self.focus_handle.clone();
            div()
                .size_full()
                .pl(self.inset)
                .track_focus(&self.focus_handle)
                .child(
                    canvas(
                        |_, _, _| {},
                        move |bounds, _, window, cx| {
                            window.handle_input(
                                &focus_handle,
                                ElementInputHandler::new(bounds, view),
                                cx,
                            );
                        },
                    )
                    .size_full(),
                )
        }
    }

    impl EntityInputHandler for ConfigurationTestView {
        fn accepts_text_input(&self, _window: &mut Window, _cx: &mut Context<Self>) -> bool {
            self.accepts_text_input
        }

        fn text_for_range(
            &mut self,
            _range: std::ops::Range<usize>,
            _adjusted_range: &mut Option<std::ops::Range<usize>>,
            _window: &mut Window,
            _cx: &mut Context<Self>,
        ) -> Option<String> {
            None
        }

        fn selected_text_range(
            &mut self,
            _ignore_disabled_input: bool,
            _window: &mut Window,
            _cx: &mut Context<Self>,
        ) -> Option<UTF16Selection> {
            self.selection
                .as_ref()
                .map(|(range, reversed)| UTF16Selection {
                    range: range.clone(),
                    reversed: *reversed,
                })
        }

        fn marked_text_range(
            &self,
            _window: &mut Window,
            _cx: &mut Context<Self>,
        ) -> Option<std::ops::Range<usize>> {
            self.marked.clone()
        }

        fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {}

        fn replace_text_in_range(
            &mut self,
            _range: Option<std::ops::Range<usize>>,
            _text: &str,
            _window: &mut Window,
            _cx: &mut Context<Self>,
        ) {
        }

        fn replace_and_mark_text_in_range(
            &mut self,
            _range: Option<std::ops::Range<usize>>,
            _new_text: &str,
            _new_selected_range: Option<std::ops::Range<usize>>,
            _window: &mut Window,
            _cx: &mut Context<Self>,
        ) {
        }

        fn bounds_for_range(
            &mut self,
            range_utf16: std::ops::Range<usize>,
            _element_bounds: Bounds<Pixels>,
            _window: &mut Window,
            _cx: &mut Context<Self>,
        ) -> Option<Bounds<Pixels>> {
            self.selection.as_ref().map(|_| {
                Bounds::new(
                    crate::point(crate::px(range_utf16.start as f32), crate::px(20.)),
                    crate::size(crate::px(1.), crate::px(12.)),
                )
            })
        }

        fn character_index_for_point(
            &mut self,
            _point: crate::Point<Pixels>,
            _window: &mut Window,
            _cx: &mut Context<Self>,
        ) -> Option<usize> {
            None
        }

        fn text_input_configuration(
            &mut self,
            _window: &mut Window,
            _cx: &mut Context<Self>,
        ) -> TextInputConfiguration {
            self.configuration.clone()
        }
    }
}
