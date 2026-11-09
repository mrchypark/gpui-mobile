use gpui::{AppContext, Context, IntoElement, Render, TestAppContext, Window, div};
use std::{cell::Cell, rc::Rc};

#[gpui::test]
fn ely_initializes_shortcuts(cx: &mut TestAppContext) {
    cx.update(ely_gpui_component::init_for_tests);
}

struct Redraw {
    armed: Rc<Cell<bool>>,
    renders: Rc<Cell<usize>>,
}

impl Render for Redraw {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        if self.armed.replace(false) {
            cx.notify();
        }
        div()
    }
}

#[gpui::test]
fn notification_during_draw_requests_another_frame(cx: &mut TestAppContext) {
    let armed = Rc::new(Cell::new(false));
    let renders = Rc::new(Cell::new(0));
    let observers = Rc::new(Cell::new(0));
    let window = cx.add_window({
        let (armed, renders, observers) = (armed.clone(), renders.clone(), observers.clone());
        move |_, cx| {
            cx.observe_self(move |_, _| observers.set(observers.get() + 1))
                .detach();
            Redraw { armed, renders }
        }
    });
    let before = renders.get();
    armed.set(true);
    cx.update_window(window.into(), |_, window, cx| {
        window.refresh();
        window.draw(cx).clear(cx)
    })
        .unwrap();
    assert_eq!(renders.get(), before + 2);
    assert_eq!(observers.get(), 1);
}
