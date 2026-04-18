use std::{cell::RefCell, mem, ops::Range, rc::Rc};

use gpui::{
    fill, point, px, size, AnyTooltip, AnyView, App, Bounds, ClipboardItem, DispatchPhase, Element,
    ElementId, GlobalElementId, Hitbox, HitboxBehavior, InspectorElementId, IntoElement,
    KeyDownEvent, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad,
    Pixels, SharedString, StyledText, TextLayout, TextRun, Window, WrappedLineLayout,
};

use crate::{
    state::AppState,
    theme::{accent_muted, fg_subtle},
};

thread_local! {
    static ACTIVE_TEXT_TARGET: RefCell<Option<String>> = const { RefCell::new(None) };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppTextFieldKind {
    PaletteQuery,
    ReviewBody,
}

#[derive(Default)]
struct TextSelectionState {
    anchor_index: Option<usize>,
    head_index: Option<usize>,
    mouse_down_index: Option<usize>,
    selecting: bool,
    hovered_index: Option<usize>,
}

impl TextSelectionState {
    fn clamp(&mut self, len: usize) {
        self.anchor_index = self.anchor_index.map(|index| index.min(len));
        self.head_index = self.head_index.map(|index| index.min(len));
        self.mouse_down_index = self.mouse_down_index.map(|index| index.min(len));
    }

    fn selection_range(&self) -> Option<Range<usize>> {
        let anchor = self.anchor_index?;
        let head = self.head_index.unwrap_or(anchor);
        Some(anchor.min(head)..anchor.max(head))
    }

    fn cursor_index(&self) -> usize {
        self.head_index.or(self.anchor_index).unwrap_or(0)
    }

    fn collapse_to(&mut self, index: usize) {
        self.anchor_index = Some(index);
        self.head_index = Some(index);
    }

    fn select_to(&mut self, index: usize) {
        if self.anchor_index.is_none() {
            self.anchor_index = Some(index);
        }
        self.head_index = Some(index);
    }

    fn select_all(&mut self, len: usize) {
        self.anchor_index = Some(0);
        self.head_index = Some(len);
    }

    fn clear(&mut self) {
        self.anchor_index = None;
        self.head_index = None;
        self.mouse_down_index = None;
        self.selecting = false;
    }
}

struct SelectableTextClickEvent {
    mouse_down_index: usize,
    mouse_up_index: usize,
}

#[doc(hidden)]
#[derive(Default)]
pub struct SelectableTextState {
    selection: Rc<RefCell<TextSelectionState>>,
}

pub struct SelectableText {
    element_id: ElementId,
    selection_id: String,
    raw_text: SharedString,
    text: StyledText,
    click_listener:
        Option<Box<dyn Fn(&[Range<usize>], SelectableTextClickEvent, &mut Window, &mut App)>>,
    hover_listener: Option<Box<dyn Fn(Option<usize>, MouseMoveEvent, &mut Window, &mut App)>>,
    tooltip_builder: Option<Rc<dyn Fn(usize, &mut Window, &mut App) -> Option<AnyView>>>,
    clickable_ranges: Vec<Range<usize>>,
    selection_color: gpui::Rgba,
}

impl SelectableText {
    pub fn new(id: impl Into<SharedString>, text: impl Into<SharedString>) -> Self {
        let raw_text = text.into();
        let selection_id: SharedString = id.into();
        let element_id = ElementId::Name(selection_id.clone());

        Self {
            element_id,
            selection_id: selection_id.to_string(),
            text: StyledText::new(raw_text.clone()),
            raw_text,
            click_listener: None,
            hover_listener: None,
            tooltip_builder: None,
            clickable_ranges: Vec::new(),
            selection_color: accent_muted(),
        }
    }

    pub fn with_runs(mut self, runs: Vec<TextRun>) -> Self {
        self.text = StyledText::new(self.raw_text.clone()).with_runs(runs);
        self
    }

    pub fn on_click(
        mut self,
        ranges: Vec<Range<usize>>,
        listener: impl Fn(usize, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.click_listener = Some(Box::new(move |ranges, event, window, cx| {
            for (range_ix, range) in ranges.iter().enumerate() {
                if range.contains(&event.mouse_down_index) && range.contains(&event.mouse_up_index)
                {
                    listener(range_ix, window, cx);
                }
            }
        }));
        self.clickable_ranges = ranges;
        self
    }

    pub fn on_hover(
        mut self,
        listener: impl Fn(Option<usize>, MouseMoveEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.hover_listener = Some(Box::new(listener));
        self
    }

    pub fn tooltip(
        mut self,
        builder: impl Fn(usize, &mut Window, &mut App) -> Option<AnyView> + 'static,
    ) -> Self {
        self.tooltip_builder = Some(Rc::new(builder));
        self
    }
}

impl Element for SelectableText {
    type RequestLayoutState = ();
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        Some(self.element_id.clone())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        self.text.request_layout(None, inspector_id, window, cx)
    }

    fn prepaint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Hitbox {
        window.with_optional_element_state::<SelectableTextState, _>(
            global_id,
            |selectable_state, window| {
                let selectable_state =
                    selectable_state.map(|selectable_state| selectable_state.unwrap_or_default());

                self.text
                    .prepaint(None, inspector_id, bounds, state, window, cx);
                let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);

                if let Some(tooltip_builder) = self.tooltip_builder.clone() {
                    let selection_state = selectable_state
                        .as_ref()
                        .map(|state| state.selection.clone())
                        .unwrap_or_default();
                    let mouse_position = window.mouse_position();
                    if bounds.contains(&mouse_position) && !selection_state.borrow().selecting {
                        if let Ok(index) = self.text.layout().index_for_position(mouse_position) {
                            if let Some(view) = tooltip_builder(index, window, cx) {
                                let source_bounds = bounds;
                                let selection_state = selection_state.clone();
                                window.set_tooltip(AnyTooltip {
                                    view,
                                    mouse_position,
                                    check_visible_and_update: Rc::new(
                                        move |_tooltip_bounds, window, _cx| {
                                            source_bounds.contains(&window.mouse_position())
                                                && !selection_state.borrow().selecting
                                        },
                                    ),
                                });
                            }
                        }
                    }
                }

                (hitbox, selectable_state)
            },
        )
    }

    fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        hitbox: &mut Hitbox,
        window: &mut Window,
        cx: &mut App,
    ) {
        let text_layout = self.text.layout().clone();
        let selection_id = self.selection_id.clone();
        let raw_text = self.raw_text.clone();

        window.with_element_state::<SelectableTextState, _>(
            global_id.unwrap(),
            |selectable_state, window| {
                let selectable_state = selectable_state.unwrap_or_default();
                let selection_state = selectable_state.selection.clone();
                selection_state.borrow_mut().clamp(raw_text.len());

                if let Some(hover_listener) = self.hover_listener.take() {
                    let hover_selection = selection_state.clone();
                    let hover_hitbox = hitbox.clone();
                    let hover_layout = text_layout.clone();
                    window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                        if phase != DispatchPhase::Bubble {
                            return;
                        }

                        let hovered = hover_hitbox.is_hovered(window).then(|| {
                            hover_layout
                                .index_for_position(event.position)
                                .unwrap_or_else(|index| index)
                        });

                        if hover_selection.borrow().hovered_index == hovered {
                            return;
                        }

                        hover_selection.borrow_mut().hovered_index = hovered;
                        hover_listener(hovered, event.clone(), window, cx);
                        window.refresh();
                    });
                }

                let clear_selection_hitbox = hitbox.clone();
                let clear_selection_id = selection_id.clone();
                let clear_selection_state = selection_state.clone();
                window.on_mouse_event(move |_event: &MouseDownEvent, phase, window, _cx| {
                    if phase != DispatchPhase::Capture {
                        return;
                    }
                    if !is_active_text_target(&clear_selection_id) {
                        return;
                    }
                    if clear_selection_hitbox.is_hovered(window) {
                        return;
                    }

                    clear_selection_state.borrow_mut().clear();
                    clear_active_text_target(&clear_selection_id);
                    window.refresh();
                });

                let mouse_down_hitbox = hitbox.clone();
                let mouse_down_layout = text_layout.clone();
                let mouse_down_selection = selection_state.clone();
                let mouse_down_id = selection_id.clone();
                window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                    if phase != DispatchPhase::Capture
                        || event.button != MouseButton::Left
                        || !mouse_down_hitbox.is_hovered(window)
                    {
                        return;
                    }

                    let index = mouse_down_layout
                        .index_for_position(event.position)
                        .unwrap_or_else(|index| index);

                    {
                        let mut state = mouse_down_selection.borrow_mut();
                        state.mouse_down_index = Some(index);
                        state.selecting = true;
                        if event.modifiers.shift && state.anchor_index.is_some() {
                            state.select_to(index);
                        } else {
                            state.collapse_to(index);
                        }
                    }

                    set_active_text_target(mouse_down_id.clone());
                    cx.stop_propagation();
                    window.refresh();
                });

                let mouse_move_selection = selection_state.clone();
                let mouse_move_layout = text_layout.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, _cx| {
                    if phase != DispatchPhase::Bubble {
                        return;
                    }

                    if !mouse_move_selection.borrow().selecting {
                        return;
                    }

                    let index = mouse_move_layout
                        .index_for_position(event.position)
                        .unwrap_or_else(|index| index);
                    mouse_move_selection.borrow_mut().select_to(index);
                    window.refresh();
                });

                let mouse_up_selection = selection_state.clone();
                let mouse_up_layout = text_layout.clone();
                let click_ranges = mem::take(&mut self.clickable_ranges);
                let click_listener = self.click_listener.take();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                    if phase != DispatchPhase::Capture || event.button != MouseButton::Left {
                        return;
                    }

                    let maybe_mouse_down = mouse_up_selection.borrow().mouse_down_index;
                    if maybe_mouse_down.is_none() && !mouse_up_selection.borrow().selecting {
                        return;
                    }

                    let mouse_up_index = mouse_up_layout
                        .index_for_position(event.position)
                        .unwrap_or_else(|index| index);

                    let mut state = mouse_up_selection.borrow_mut();
                    if state.selecting {
                        state.select_to(mouse_up_index);
                    }
                    state.selecting = false;
                    let mouse_down_index = state.mouse_down_index.take();
                    let selection_range = state.selection_range();
                    drop(state);

                    if let (Some(mouse_down_index), Some(listener)) =
                        (mouse_down_index, click_listener.as_ref())
                    {
                        let collapsed = selection_range
                            .as_ref()
                            .map(|range| range.is_empty())
                            .unwrap_or(false);
                        if collapsed {
                            listener(
                                &click_ranges,
                                SelectableTextClickEvent {
                                    mouse_down_index,
                                    mouse_up_index,
                                },
                                window,
                                cx,
                            );
                        }
                    }

                    cx.stop_propagation();
                    window.refresh();
                });

                window.on_key_event({
                    let key_selection = selection_state.clone();
                    let key_id = selection_id.clone();
                    let key_text = raw_text.clone();
                    move |event: &KeyDownEvent, phase, _window, cx| {
                        if phase != DispatchPhase::Bubble || !is_active_text_target(&key_id) {
                            return;
                        }

                        let modifiers = event.keystroke.modifiers;
                        let platform_only = platform_primary_modifier(modifiers) && !modifiers.alt;
                        match event.keystroke.key.as_str() {
                            "a" if platform_only && !key_text.is_empty() => {
                                key_selection.borrow_mut().select_all(key_text.len());
                                cx.stop_propagation();
                            }
                            "c" if platform_only => {
                                if let Some(range) = key_selection.borrow().selection_range() {
                                    if !range.is_empty() {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            key_text[range].to_string(),
                                        ));
                                        cx.stop_propagation();
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                });

                let hovered_index = selection_state.borrow().hovered_index;
                if let Some(index) = hovered_index {
                    if self
                        .clickable_ranges
                        .iter()
                        .any(|range| range.contains(&index))
                    {
                        window.set_cursor_style(gpui::CursorStyle::PointingHand, hitbox);
                    } else {
                        window.set_cursor_style(gpui::CursorStyle::IBeam, hitbox);
                    }
                } else if hitbox.is_hovered(window) {
                    window.set_cursor_style(gpui::CursorStyle::IBeam, hitbox);
                }

                if let Some(range) = selection_state.borrow().selection_range() {
                    for quad in selection_quads_for_range(
                        raw_text.as_ref(),
                        &text_layout,
                        range,
                        self.selection_color,
                    ) {
                        window.paint_quad(quad);
                    }
                }

                self.text
                    .paint(None, inspector_id, bounds, &mut (), &mut (), window, cx);

                ((), selectable_state)
            },
        );
    }
}

impl IntoElement for SelectableText {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

#[doc(hidden)]
#[derive(Default)]
pub struct AppTextInputState {
    selection: Rc<RefCell<TextSelectionState>>,
}

pub struct AppTextInput {
    element_id: ElementId,
    selection_id: String,
    state: gpui::Entity<AppState>,
    field: AppTextFieldKind,
    placeholder: SharedString,
    text: StyledText,
    raw_text: SharedString,
    display_text: SharedString,
    autofocus: bool,
    multiline: bool,
    selection_color: gpui::Rgba,
}

impl AppTextInput {
    pub fn new(
        id: impl Into<SharedString>,
        state: gpui::Entity<AppState>,
        field: AppTextFieldKind,
        placeholder: impl Into<SharedString>,
    ) -> Self {
        let selection_id: SharedString = id.into();
        let element_id = ElementId::Name(selection_id.clone());
        let placeholder = placeholder.into();

        Self {
            element_id,
            selection_id: selection_id.to_string(),
            state,
            field,
            placeholder: placeholder.clone(),
            text: StyledText::new(SharedString::new("")),
            raw_text: SharedString::new(""),
            display_text: placeholder,
            autofocus: false,
            multiline: matches!(field, AppTextFieldKind::ReviewBody),
            selection_color: accent_muted(),
        }
    }

    pub fn autofocus(mut self, autofocus: bool) -> Self {
        self.autofocus = autofocus;
        self
    }

    fn sync_content(&mut self, cx: &App) {
        let raw_text = {
            let app_state = self.state.read(cx);
            match self.field {
                AppTextFieldKind::PaletteQuery => app_state.palette_query.clone(),
                AppTextFieldKind::ReviewBody => app_state.review_body.clone(),
            }
        };

        self.raw_text = raw_text.clone().into();
        self.display_text = if raw_text.is_empty() {
            self.placeholder.clone()
        } else {
            self.raw_text.clone()
        };
        self.text = StyledText::new(self.display_text.clone());
    }
}

impl Element for AppTextInput {
    type RequestLayoutState = ();
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        Some(self.element_id.clone())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        self.sync_content(cx);
        self.text.request_layout(None, inspector_id, window, cx)
    }

    fn prepaint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Hitbox {
        window.with_optional_element_state::<AppTextInputState, _>(
            global_id,
            |input_state, window| {
                let input_state = input_state.map(|input_state| input_state.unwrap_or_default());
                self.text
                    .prepaint(None, inspector_id, bounds, state, window, cx);
                let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
                (hitbox, input_state)
            },
        )
    }

    fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        hitbox: &mut Hitbox,
        window: &mut Window,
        cx: &mut App,
    ) {
        let text_layout = self.text.layout().clone();
        let raw_text = self.raw_text.clone();
        let selection_id = self.selection_id.clone();
        let field = self.field;
        let state = self.state.clone();
        let multiline = self.multiline;
        let autofocus = self.autofocus;

        window.with_element_state::<AppTextInputState, _>(
            global_id.unwrap(),
            |input_state, window| {
                let input_state = input_state.unwrap_or_default();
                let selection_state = input_state.selection.clone();
                selection_state.borrow_mut().clamp(raw_text.len());

                if autofocus {
                    set_active_text_target(selection_id.clone());
                }

                let clear_hitbox = hitbox.clone();
                let clear_selection_state = selection_state.clone();
                let clear_selection_id = selection_id.clone();
                window.on_mouse_event(move |_event: &MouseDownEvent, phase, window, _cx| {
                    if phase != DispatchPhase::Capture {
                        return;
                    }
                    if !is_active_text_target(&clear_selection_id)
                        || clear_hitbox.is_hovered(window)
                    {
                        return;
                    }

                    clear_active_text_target(&clear_selection_id);
                    clear_selection_state.borrow_mut().clear();
                    window.refresh();
                });

                let mouse_down_hitbox = hitbox.clone();
                let mouse_down_layout = text_layout.clone();
                let mouse_down_selection = selection_state.clone();
                let mouse_down_state = state.clone();
                let mouse_down_id = selection_id.clone();
                let mouse_down_text = raw_text.clone();
                window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                    if phase != DispatchPhase::Capture
                        || event.button != MouseButton::Left
                        || !mouse_down_hitbox.is_hovered(window)
                    {
                        return;
                    }

                    let index = if mouse_down_text.is_empty() {
                        0
                    } else {
                        mouse_down_layout
                            .index_for_position(event.position)
                            .unwrap_or_else(|index| index)
                    };

                    mouse_down_state.update(cx, |app_state, cx| {
                        if matches!(field, AppTextFieldKind::ReviewBody) {
                            app_state.review_editor_active = true;
                            app_state.review_message = None;
                            app_state.review_success = false;
                        }
                        cx.notify();
                    });

                    {
                        let mut selection = mouse_down_selection.borrow_mut();
                        selection.mouse_down_index = Some(index);
                        selection.selecting = true;
                        if event.modifiers.shift && selection.anchor_index.is_some() {
                            selection.select_to(index);
                        } else {
                            selection.collapse_to(index);
                        }
                    }

                    set_active_text_target(mouse_down_id.clone());
                    cx.stop_propagation();
                    window.refresh();
                });

                let mouse_move_layout = text_layout.clone();
                let mouse_move_selection = selection_state.clone();
                let mouse_move_text = raw_text.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, _cx| {
                    if phase != DispatchPhase::Bubble || !mouse_move_selection.borrow().selecting {
                        return;
                    }

                    let index = if mouse_move_text.is_empty() {
                        0
                    } else {
                        mouse_move_layout
                            .index_for_position(event.position)
                            .unwrap_or_else(|index| index)
                    };

                    mouse_move_selection.borrow_mut().select_to(index);
                    window.refresh();
                });

                let mouse_up_selection = selection_state.clone();
                window.on_mouse_event(move |_event: &MouseUpEvent, phase, window, cx| {
                    if phase != DispatchPhase::Capture {
                        return;
                    }
                    if !mouse_up_selection.borrow().selecting {
                        return;
                    }

                    let mut selection = mouse_up_selection.borrow_mut();
                    selection.selecting = false;
                    selection.mouse_down_index = None;
                    drop(selection);

                    cx.stop_propagation();
                    window.refresh();
                });

                window.on_key_event({
                    let key_state = state.clone();
                    let key_selection = selection_state.clone();
                    let key_id = selection_id.clone();
                    let key_text = raw_text.clone();
                    move |event: &KeyDownEvent, phase, window, cx| {
                        if phase != DispatchPhase::Bubble || !is_active_text_target(&key_id) {
                            return;
                        }

                        let modifiers = event.keystroke.modifiers;
                        let platform_only = platform_primary_modifier(modifiers) && !modifiers.alt;
                        let key = event.keystroke.key.as_str();

                        let mut handled = true;
                        match key {
                            "left" => {
                                key_state.update(cx, |app_state, cx| {
                                    move_input_selection(
                                        input_text_for_field(app_state, field),
                                        &key_selection,
                                        MoveDirection::Left,
                                        modifiers.shift,
                                    );
                                    cx.notify();
                                });
                            }
                            "right" => {
                                key_state.update(cx, |app_state, cx| {
                                    move_input_selection(
                                        input_text_for_field(app_state, field),
                                        &key_selection,
                                        MoveDirection::Right,
                                        modifiers.shift,
                                    );
                                    cx.notify();
                                });
                            }
                            "home" => {
                                key_selection
                                    .borrow_mut()
                                    .select_to_or_collapse(0, modifiers.shift);
                                window.refresh();
                            }
                            "end" => {
                                let len = key_text.len();
                                key_selection
                                    .borrow_mut()
                                    .select_to_or_collapse(len, modifiers.shift);
                                window.refresh();
                            }
                            "backspace" => {
                                key_state.update(cx, |app_state, cx| {
                                    edit_input_text(
                                        app_state,
                                        field,
                                        &key_selection,
                                        EditCommand::Backspace,
                                    );
                                    cx.notify();
                                });
                            }
                            "delete" => {
                                key_state.update(cx, |app_state, cx| {
                                    edit_input_text(
                                        app_state,
                                        field,
                                        &key_selection,
                                        EditCommand::Delete,
                                    );
                                    cx.notify();
                                });
                            }
                            "a" if platform_only => {
                                key_selection.borrow_mut().select_all(key_text.len());
                                window.refresh();
                            }
                            "c" if platform_only => {
                                if let Some(range) = key_selection.borrow().selection_range() {
                                    if !range.is_empty() {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            key_text[range].to_string(),
                                        ));
                                    }
                                }
                            }
                            "x" if platform_only => {
                                key_state.update(cx, |app_state, cx| {
                                    cut_input_text(app_state, field, &key_selection, cx);
                                    cx.notify();
                                });
                            }
                            "v" if platform_only => {
                                if let Some(text) =
                                    cx.read_from_clipboard().and_then(|item| item.text())
                                {
                                    key_state.update(cx, |app_state, cx| {
                                        edit_input_text(
                                            app_state,
                                            field,
                                            &key_selection,
                                            EditCommand::Insert(normalize_paste(field, &text)),
                                        );
                                        cx.notify();
                                    });
                                }
                            }
                            "enter" if !platform_only && multiline => {
                                key_state.update(cx, |app_state, cx| {
                                    edit_input_text(
                                        app_state,
                                        field,
                                        &key_selection,
                                        EditCommand::Insert("\n".to_string()),
                                    );
                                    cx.notify();
                                });
                            }
                            _ => {
                                handled = false;
                                if let Some(input) = event.keystroke.key_char.as_ref() {
                                    if should_insert_text(input, multiline) {
                                        let input = if multiline {
                                            input.to_string()
                                        } else {
                                            input.replace('\n', " ")
                                        };
                                        key_state.update(cx, |app_state, cx| {
                                            edit_input_text(
                                                app_state,
                                                field,
                                                &key_selection,
                                                EditCommand::Insert(input.clone()),
                                            );
                                            cx.notify();
                                        });
                                        handled = true;
                                    }
                                }
                            }
                        }

                        if handled {
                            cx.stop_propagation();
                        }
                    }
                });

                if hitbox.is_hovered(window) || is_active_text_target(&selection_id) {
                    window.set_cursor_style(gpui::CursorStyle::IBeam, hitbox);
                }

                if let Some(range) = selection_state.borrow().selection_range() {
                    for quad in selection_quads_for_range(
                        raw_text.as_ref(),
                        &text_layout,
                        range,
                        self.selection_color,
                    ) {
                        window.paint_quad(quad);
                    }
                }

                if is_active_text_target(&selection_id) {
                    if let Some(cursor_quad) =
                        cursor_quad_for_index(&text_layout, selection_state.borrow().cursor_index())
                    {
                        window.paint_quad(cursor_quad);
                    }
                }

                self.text
                    .paint(None, inspector_id, bounds, &mut (), &mut (), window, cx);

                ((), input_state)
            },
        );
    }
}

impl IntoElement for AppTextInput {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl TextSelectionState {
    fn select_to_or_collapse(&mut self, index: usize, extend: bool) {
        if extend {
            self.select_to(index);
        } else {
            self.collapse_to(index);
        }
    }
}

#[derive(Clone, Copy)]
enum MoveDirection {
    Left,
    Right,
}

enum EditCommand {
    Insert(String),
    Backspace,
    Delete,
}

fn move_input_selection(
    text: &str,
    selection: &Rc<RefCell<TextSelectionState>>,
    direction: MoveDirection,
    extend: bool,
) {
    let mut selection = selection.borrow_mut();
    selection.clamp(text.len());

    let target = if extend {
        match direction {
            MoveDirection::Left => previous_boundary(text, selection.cursor_index()),
            MoveDirection::Right => next_boundary(text, selection.cursor_index()),
        }
    } else if let Some(range) = selection.selection_range() {
        if !range.is_empty() {
            match direction {
                MoveDirection::Left => range.start,
                MoveDirection::Right => range.end,
            }
        } else {
            match direction {
                MoveDirection::Left => previous_boundary(text, selection.cursor_index()),
                MoveDirection::Right => next_boundary(text, selection.cursor_index()),
            }
        }
    } else {
        match direction {
            MoveDirection::Left => previous_boundary(text, selection.cursor_index()),
            MoveDirection::Right => next_boundary(text, selection.cursor_index()),
        }
    };

    selection.select_to_or_collapse(target, extend);
}

fn input_text_for_field<'a>(state: &'a AppState, field: AppTextFieldKind) -> &'a str {
    match field {
        AppTextFieldKind::PaletteQuery => state.palette_query.as_str(),
        AppTextFieldKind::ReviewBody => state.review_body.as_str(),
    }
}

fn set_input_text_for_field(state: &mut AppState, field: AppTextFieldKind, value: String) {
    match field {
        AppTextFieldKind::PaletteQuery => {
            state.palette_query = value;
            state.palette_selected_index = 0;
        }
        AppTextFieldKind::ReviewBody => {
            state.review_body = value;
        }
    }
}

fn cut_input_text(
    state: &mut AppState,
    field: AppTextFieldKind,
    selection: &Rc<RefCell<TextSelectionState>>,
    cx: &mut App,
) {
    let text = input_text_for_field(state, field).to_string();
    let range = selection.borrow().selection_range().unwrap_or(0..0);
    if range.is_empty() {
        return;
    }

    cx.write_to_clipboard(ClipboardItem::new_string(text[range.clone()].to_string()));
    apply_replacement(state, field, selection, range, "");
}

fn edit_input_text(
    state: &mut AppState,
    field: AppTextFieldKind,
    selection: &Rc<RefCell<TextSelectionState>>,
    command: EditCommand,
) {
    let text = input_text_for_field(state, field).to_string();
    let mut selection_state = selection.borrow_mut();
    selection_state.clamp(text.len());

    let selection_range = selection_state.selection_range().unwrap_or_else(|| {
        let cursor = selection_state.cursor_index();
        cursor..cursor
    });

    match command {
        EditCommand::Insert(new_text) => {
            drop(selection_state);
            apply_replacement(state, field, selection, selection_range, &new_text);
        }
        EditCommand::Backspace => {
            let delete_range = if selection_range.is_empty() {
                let cursor = selection_range.end;
                previous_boundary(&text, cursor)..cursor
            } else {
                selection_range
            };
            drop(selection_state);
            apply_replacement(state, field, selection, delete_range, "");
        }
        EditCommand::Delete => {
            let delete_range = if selection_range.is_empty() {
                let cursor = selection_range.end;
                cursor..next_boundary(&text, cursor)
            } else {
                selection_range
            };
            drop(selection_state);
            apply_replacement(state, field, selection, delete_range, "");
        }
    }
}

fn apply_replacement(
    state: &mut AppState,
    field: AppTextFieldKind,
    selection: &Rc<RefCell<TextSelectionState>>,
    range: Range<usize>,
    replacement: &str,
) {
    let text = input_text_for_field(state, field).to_string();
    let mut next = String::with_capacity(text.len() + replacement.len());
    next.push_str(&text[..range.start]);
    next.push_str(replacement);
    next.push_str(&text[range.end..]);
    set_input_text_for_field(state, field, next);

    let cursor = range.start + replacement.len();
    let mut selection_state = selection.borrow_mut();
    selection_state.collapse_to(cursor);
    selection_state.clamp(input_text_for_field(state, field).len());
}

fn normalize_paste(field: AppTextFieldKind, text: &str) -> String {
    match field {
        AppTextFieldKind::PaletteQuery => text.replace('\n', " "),
        AppTextFieldKind::ReviewBody => text.to_string(),
    }
}

fn should_insert_text(input: &str, multiline: bool) -> bool {
    if input == "\t" {
        return false;
    }
    if input == "\n" {
        return multiline;
    }
    !input.chars().all(|character| character.is_control())
}

fn selection_quads_for_range(
    text: &str,
    layout: &TextLayout,
    selection: Range<usize>,
    color: gpui::Rgba,
) -> Vec<PaintQuad> {
    if selection.is_empty() || text.is_empty() {
        return Vec::new();
    }

    let bounds = layout.bounds();
    let line_height = layout.line_height();
    let hard_lines = text.split('\n').collect::<Vec<_>>();
    let mut line_start = 0usize;
    let mut block_y = Pixels::ZERO;
    let mut quads = Vec::new();

    for (line_ix, line_text) in hard_lines.iter().enumerate() {
        let line_query_index = line_start.min(layout.len().saturating_sub(1));
        let Some(line_layout) = layout.line_layout_for_index(line_query_index) else {
            line_start += line_text.len();
            if line_ix + 1 < hard_lines.len() {
                line_start += 1;
            }
            continue;
        };

        let segment_ends = wrapped_segment_end_indices(&line_layout);
        let mut segment_start = 0usize;
        for segment_end in segment_ends {
            let global_segment_start = line_start + segment_start;
            let global_segment_end = line_start + segment_end;
            let overlap_start = selection.start.max(global_segment_start);
            let overlap_end = selection.end.min(global_segment_end);
            if overlap_start < overlap_end {
                let local_start = overlap_start - line_start;
                let local_end = overlap_end - line_start;
                if let (Some(start), Some(end)) = (
                    line_layout.position_for_index(local_start, line_height),
                    line_layout.position_for_index(local_end, line_height),
                ) {
                    let top = bounds.top() + block_y + start.y;
                    let bottom = top + line_height;
                    let left = bounds.left() + start.x;
                    let right = bounds.left() + end.x.max(start.x + px(1.0));
                    quads.push(fill(
                        Bounds::from_corners(point(left, top), point(right, bottom)),
                        color,
                    ));
                }
            }
            segment_start = segment_end;
        }

        block_y += line_layout.size(line_height).height;
        line_start += line_text.len();
        if line_ix + 1 < hard_lines.len() {
            line_start += 1;
        }
    }

    quads
}

fn wrapped_segment_end_indices(layout: &WrappedLineLayout) -> Vec<usize> {
    let mut ends = layout
        .wrap_boundaries()
        .iter()
        .map(|boundary| {
            let run = &layout.runs()[boundary.run_ix];
            let glyph = &run.glyphs[boundary.glyph_ix];
            glyph.index
        })
        .collect::<Vec<_>>();
    ends.push(layout.len());
    ends
}

fn cursor_quad_for_index(layout: &TextLayout, index: usize) -> Option<PaintQuad> {
    let position = layout.position_for_index(index)?;
    let bounds = layout.bounds();
    let line_height = layout.line_height();
    Some(fill(
        Bounds::new(
            point(bounds.left() + position.x, bounds.top() + position.y),
            size(px(2.0), line_height),
        ),
        fg_subtle(),
    ))
}

fn previous_boundary(text: &str, offset: usize) -> usize {
    text.char_indices()
        .rev()
        .find_map(|(index, _)| (index < offset).then_some(index))
        .unwrap_or(0)
}

fn next_boundary(text: &str, offset: usize) -> usize {
    text.char_indices()
        .find_map(|(index, _)| (index > offset).then_some(index))
        .unwrap_or(text.len())
}

fn platform_primary_modifier(modifiers: gpui::Modifiers) -> bool {
    modifiers.platform
}

fn set_active_text_target(id: String) {
    ACTIVE_TEXT_TARGET.with(|active| {
        active.replace(Some(id));
    });
}

fn clear_active_text_target(id: &str) {
    ACTIVE_TEXT_TARGET.with(|active| {
        let should_clear = active.borrow().as_deref() == Some(id);
        if should_clear {
            active.replace(None);
        }
    });
}

fn is_active_text_target(id: &str) -> bool {
    ACTIVE_TEXT_TARGET.with(|active| active.borrow().as_deref() == Some(id))
}
