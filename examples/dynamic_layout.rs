use druid::lens::Map;
use druid::widget::{Axis, Button, Flex, Label, Stepper};
use druid::{
    AppLauncher, BoxConstraints, Color, Data, Env, Event, EventCtx, LayoutCtx, Lens, LensExt,
    LifeCycle, LifeCycleCtx, PaintCtx, Point, Size, UpdateCtx, Widget, WidgetExt, WidgetPod,
    WindowDesc, WindowSizePolicy,
};
use druid_widget_nursery::EnsuredPool;
use std::cell::RefCell;
use std::hash::Hash;
use std::marker::PhantomData;
use std::rc::Rc;
use std::time::Instant;

struct Inner<T: Data> {
    added: bool,
    widget: Box<dyn Widget<T>>,
}

#[derive(Clone)]
struct SlotWidget<T: Data> {
    child: Rc<RefCell<Inner<T>>>,
}

impl<T: Data> SlotWidget<T> {
    pub fn new(w: impl Widget<T> + 'static) -> Self {
        SlotWidget {
            child: Rc::new(RefCell::new(Inner {
                added: false,
                widget: w.boxed(),
            })),
        }
    }
}

impl<T: Data> Widget<T> for SlotWidget<T> {
    fn event(&mut self, ctx: &mut EventCtx, event: &Event, data: &mut T, env: &Env) {
        let mut inner = self.child.borrow_mut();
        inner.widget.event(ctx, event, data, env);
    }

    fn lifecycle(&mut self, ctx: &mut LifeCycleCtx, event: &LifeCycle, data: &T, env: &Env) {
        let mut inner = self.child.borrow_mut();
        if let LifeCycle::WidgetAdded = event {
            if inner.added {
                return;
            }
            inner.added = true
        }
        inner.widget.lifecycle(ctx, event, data, env)
    }

    fn update(&mut self, ctx: &mut UpdateCtx, old_data: &T, data: &T, env: &Env) {
        let mut inner = self.child.borrow_mut();
        inner.widget.update(ctx, old_data, data, env)
    }

    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints, data: &T, env: &Env) -> Size {
        let mut inner = self.child.borrow_mut();
        inner.widget.layout(ctx, bc, data, env)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, data: &T, env: &Env) {
        let mut inner = self.child.borrow_mut();
        inner.widget.paint(ctx, data, env)
    }
}

struct MaybeUpdate<'a, 'b, 'c, T> {
    ctx: &'c mut UpdateCtx<'a, 'b>,
    old_data: &'c T,
    data: &'c T,
    env: &'c Env,
}

struct Slots<'a, 'b, 'c, K: Hash + Eq, T: Data> {
    update: Option<MaybeUpdate<'a, 'b, 'c, T>>,
    pool: &'c mut EnsuredPool<K, SlotWidget<T>>,
    phantom_k: PhantomData<*const K>,
    phantom_t: PhantomData<*const T>,
}

impl<'a, 'b, 'c, K: Hash + Eq + Clone, T: Data> Slots<'a, 'b, 'c, K, T> {
    fn new(
        update: Option<MaybeUpdate<'a, 'b, 'c, T>>,
        pool: &'c mut EnsuredPool<K, SlotWidget<T>>,
    ) -> Self {
        Self {
            update,
            pool,
            phantom_k: PhantomData,
            phantom_t: PhantomData,
        }
    }

    fn ensure<W: Widget<T> + 'static>(&mut self, key: K, make: impl Fn() -> W) -> SlotWidget<T> {
        let changed = self.pool.ensure(
            std::iter::once(&key),
            |k| k,
            move |_| SlotWidget::new(make()),
        );

        let found = self.pool.get(&key).unwrap();
        if !changed {
            if let Some(mu) = &mut self.update {
                let mut inner = found.child.borrow_mut();
                inner.widget.update(mu.ctx, mu.old_data, mu.data, mu.env)
            }
        }

        (*found).clone()
    }
}

type LayoutFactory<K, T> = dyn Fn(&T, &mut Slots<K, T>) -> Box<dyn Widget<T>>;
type LayoutChanged<T> = dyn Fn(&T, &T) -> bool;

struct DynamicLayout<K: Hash + Eq, T: Data> {
    changed: Box<LayoutChanged<T>>,
    layout: Box<LayoutFactory<K, T>>,
    pool: EnsuredPool<K, SlotWidget<T>>,
    child: Option<WidgetPod<T, Box<dyn Widget<T>>>>,
}

impl<K: Hash + Eq + Clone, T: Data> DynamicLayout<K, T> {
    pub fn new<W: Widget<T> + 'static>(
        changed: impl Fn(&T, &T) -> bool + 'static,
        layout: impl Fn(&T, &mut Slots<K, T>) -> W + 'static,
    ) -> Self {
        DynamicLayout {
            changed: Box::new(changed),
            layout: Box::new(move |t, repo| layout(t, repo).boxed()),
            pool: EnsuredPool::default().with_load_factor(1000.0),
            child: None,
        }
    }
}

impl<K: Hash + Eq + Clone, T: Data> Widget<T> for DynamicLayout<K, T> {
    fn event(&mut self, ctx: &mut EventCtx, event: &Event, data: &mut T, env: &Env) {
        if let Some(child) = self.child.as_mut() {
            child.event(ctx, event, data, env);
        }
    }

    fn lifecycle(&mut self, ctx: &mut LifeCycleCtx, event: &LifeCycle, data: &T, env: &Env) {
        if let LifeCycle::WidgetAdded = event {
            let mut repo = Slots::new(None, &mut self.pool);
            let layout = &self.layout;
            self.child = Some(WidgetPod::new((layout)(data, &mut repo)));
        }
        if let Some(child) = self.child.as_mut() {
            child.lifecycle(ctx, event, data, env);
        }
    }

    fn update(&mut self, ctx: &mut UpdateCtx, old_data: &T, data: &T, env: &Env) {
        if let Some(child) = self.child.as_mut() {
            child.update(ctx, data, env);
        }

        if (self.changed)(old_data, data) {
            let mu = MaybeUpdate {
                ctx,
                old_data,
                data,
                env,
            };

            let mut repo = Slots::new(Some(mu), &mut self.pool);
            let layout = &self.layout;
            self.child = Some(WidgetPod::new((layout)(data, &mut repo)));
            ctx.children_changed();
        }
    }

    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints, data: &T, env: &Env) -> Size {
        match self.child {
            Some(ref mut child) => {
                let size = child.layout(ctx, bc, data, env);
                child.set_origin(ctx, data, env, Point::ORIGIN);
                size
            }
            None => bc.max(),
        }
    }

    fn paint(&mut self, ctx: &mut PaintCtx, data: &T, env: &Env) {
        if let Some(ref mut child) = self.child {
            child.paint_raw(ctx, data, env);
        }
    }
}

#[derive(Data, Clone, Lens)]
struct AppState {
    number: usize,
}

pub const COLORS: [Color; 4] = [Color::BLACK, Color::BLUE, Color::MAROON, Color::RED];
fn stateful_color() -> Color {
    COLORS[Instant::now().elapsed().as_nanos() as usize % COLORS.len()].clone()
}

fn main_widget() -> impl Widget<AppState> {
    Flex::column()
        .with_child(
            Flex::row()
                .with_child(Label::new("Labels:"))
                .with_flex_spacer(1.)
                .with_child(Label::dynamic(|len: &usize, _| format!("{}", len)))
                .with_child(Stepper::new().lens(Map::new(|x| *x as f64, |x, y| *x = y as usize)))
                .fix_width(140.)
                .lens(AppState::number),
        )
        .with_child(DynamicLayout::new(
            |old: &AppState, new: &AppState| old.number != new.number,
            |state: &AppState, slots: &mut Slots<usize, AppState>| {
                let axis = if state.number > 3 {
                    Axis::Vertical
                } else {
                    Axis::Horizontal
                };

                let mut flex = Flex::for_axis(axis).with_child(Label::new("Header"));
                for i in 0..state.number {
                    flex.add_child(slots.ensure(i, || {
                        Label::dynamic(move |len: &usize, _| format!("Label {} of {}", i, *len))
                            .background(stateful_color())
                            .lens(AppState::number)
                    }))
                }
                flex.with_child(Label::new("Footer"))
            },
        ))
}

pub fn main() {
    let main_window = WindowDesc::new(main_widget())
        .title("Dynamic layout")
        .window_size_policy(WindowSizePolicy::Content);

    // create the initial app state
    let initial_state = AppState { number: 23 };

    // start the application
    AppLauncher::with_window(main_window)
        .launch(initial_state)
        .expect("Failed to launch application");
}
