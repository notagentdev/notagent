//! Port of `packages/tui/test/layout.test.ts` (306 LOC).
//!
//! The Kitty crop case needs `encodeKitty` and follows with task 12.

use std::cell::RefCell;
use std::rc::Rc;

use notagent_tui::components::h_stack::HStack;
use notagent_tui::components::scroll_view::{
    ScrollView, ScrollViewOptions, ScrollViewScrollbar, ScrollViewState,
};
use notagent_tui::components::stack::{StackEntryOptions, StackOptions};
use notagent_tui::components::text::Text;
use notagent_tui::components::v_stack::VStack;
use notagent_tui::layout::render_layout_frame;
use notagent_tui::layout_node::StackBasis;
use notagent_tui::strip_terminal_sequences;
use notagent_tui::tui::{Component, ComponentRef, component_ref};

fn visible_lines(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .map(|line| strip_terminal_sequences(line).trim_end().to_string())
        .collect()
}

fn text(content: &str) -> ComponentRef {
    component_ref(Text::new(content, 0, 0))
}

fn entry(
    basis: Option<StackBasis>,
    grow: Option<usize>,
    shrink: Option<usize>,
) -> StackEntryOptions {
    StackEntryOptions {
        basis,
        grow,
        shrink,
        ..StackEntryOptions::default()
    }
}

/// Component rendering fixed lines and counting its renders.
struct Lines {
    lines: Vec<String>,
    render_count: Rc<RefCell<usize>>,
}

impl Component for Lines {
    fn render(&mut self, _width: usize) -> Vec<String> {
        *self.render_count.borrow_mut() += 1;
        self.lines.clone()
    }
    fn invalidate(&mut self) {}
}

#[test]
fn allocates_vertical_grow_space_deterministically() {
    let mut stack = VStack::new(StackOptions::default());
    stack.add_child_with(text("top"), entry(Some(StackBasis::Size(1)), None, Some(0)));
    stack.add_child_with(
        text("body"),
        entry(Some(StackBasis::Size(0)), Some(1), None),
    );
    let root = component_ref(stack);

    let frame = render_layout_frame(&root, 10, 4);

    assert_eq!(
        frame
            .root
            .children
            .iter()
            .map(|child| child.rect.height)
            .collect::<Vec<_>>(),
        [1, 3]
    );
    assert_eq!(visible_lines(&frame.lines), ["top", "body", "", ""]);
}

#[test]
fn does_not_render_fixed_basis_scroll_content_during_stack_measurement() {
    let render_count = Rc::new(RefCell::new(0));
    let transcript = component_ref(ScrollView::new(
        component_ref(Lines {
            lines: vec!["one".to_string(), "two".to_string(), "three".to_string()],
            render_count: Rc::clone(&render_count),
        }),
        ScrollViewOptions::default(),
    ));
    let mut root = VStack::new(StackOptions::default());
    root.add_child_with(transcript, entry(Some(StackBasis::Size(0)), Some(1), None));
    root.add_child_with(text("dock"), entry(Some(StackBasis::Auto), None, None));
    let root = component_ref(root);

    render_layout_frame(&root, 10, 3);
    assert_eq!(*render_count.borrow(), 1);
}

#[test]
fn paints_only_clipped_rows_from_very_large_scroll_content() {
    // The TS test uses a sparse 1e9 array, which JavaScript can represent
    // lazily; the property under test is the same with a dense large vector.
    let line_count = 200_000;
    let mut lines = vec![String::new(); line_count];
    lines[line_count - 4] = "before".to_string();
    lines[line_count - 3] = "visible 1".to_string();
    lines[line_count - 2] = "visible 2".to_string();
    lines[line_count - 1] = "visible 3".to_string();
    let transcript = component_ref(ScrollView::new(
        component_ref(Lines {
            lines,
            render_count: Rc::new(RefCell::new(0)),
        }),
        ScrollViewOptions {
            follow_end: true,
            ..ScrollViewOptions::default()
        },
    ));

    let frame = render_layout_frame(&transcript, 10, 3);
    assert_eq!(
        visible_lines(&frame.lines),
        ["visible 1", "visible 2", "visible 3"]
    );
}

#[test]
fn shrinks_entries_to_their_minimum_sizes() {
    let mut stack = VStack::new(StackOptions::default());
    stack.add_child_with(
        text("a1\na2\na3"),
        StackEntryOptions {
            shrink: Some(1),
            min_size: Some(1),
            ..StackEntryOptions::default()
        },
    );
    stack.add_child_with(
        text("b1\nb2\nb3"),
        StackEntryOptions {
            shrink: Some(0),
            ..StackEntryOptions::default()
        },
    );
    let root = component_ref(stack);

    let frame = render_layout_frame(&root, 10, 4);

    assert_eq!(
        frame
            .root
            .children
            .iter()
            .map(|child| child.rect.height)
            .collect::<Vec<_>>(),
        [1, 3]
    );
    assert_eq!(visible_lines(&frame.lines), ["a1", "b1", "b2", "b3"]);
}

#[test]
fn includes_nested_minimum_sizes_in_intrinsic_stack_measurement() {
    let mut dock = VStack::new(StackOptions::default());
    dock.add_child(text("top1\ntop2\ntop3"));
    dock.add_child_with(
        text("selector"),
        StackEntryOptions {
            min_size: Some(3),
            ..StackEntryOptions::default()
        },
    );
    dock.add_child(text("below"));
    dock.add_child_with(
        text("footer"),
        StackEntryOptions {
            min_size: Some(1),
            ..StackEntryOptions::default()
        },
    );

    let mut root = VStack::new(StackOptions::default());
    root.add_child_with(
        text("body"),
        StackEntryOptions {
            basis: Some(StackBasis::Size(0)),
            grow: Some(1),
            min_size: Some(1),
            ..StackEntryOptions::default()
        },
    );
    root.add_child_with(
        component_ref(dock),
        StackEntryOptions {
            basis: Some(StackBasis::Auto),
            min_size: Some(1),
            ..StackEntryOptions::default()
        },
    );
    let root = component_ref(root);

    let frame = render_layout_frame(&root, 10, 9);
    assert_eq!(
        visible_lines(&frame.lines),
        [
            "body", "top1", "top2", "top3", "selector", "", "", "below", "footer"
        ]
    );
}

#[test]
fn omits_gaps_around_invisible_entries() {
    let mut stack = VStack::new(StackOptions {
        gap: Some(1),
        align: None,
    });
    stack.add_child(text("one"));
    stack.add_child_with(
        text("hidden"),
        StackEntryOptions {
            visible: Some(Rc::new(|_| false)),
            ..StackEntryOptions::default()
        },
    );
    stack.add_child(text("two"));

    let rendered: Vec<String> = stack
        .render(10)
        .iter()
        .map(|line| line.trim_end().to_string())
        .collect();
    assert_eq!(rendered, ["one", "", "two"]);
}

#[test]
fn composes_horizontal_children_at_allocated_widths() {
    let mut stack = HStack::new(StackOptions::default());
    stack.add_child_with(
        text("left"),
        entry(Some(StackBasis::Size(6)), None, Some(0)),
    );
    stack.add_child_with(
        text("right"),
        entry(Some(StackBasis::Size(6)), None, Some(0)),
    );
    let root = component_ref(stack);

    let frame = render_layout_frame(&root, 12, 1);
    assert_eq!(visible_lines(&frame.lines), ["left  right"]);
}

#[test]
fn does_not_paint_zero_width_horizontal_children() {
    let mut stack = HStack::new(StackOptions::default());
    stack.add_child_with(
        text("hidden"),
        entry(Some(StackBasis::Size(0)), None, Some(0)),
    );
    stack.add_child_with(
        text("shown"),
        entry(Some(StackBasis::Size(0)), Some(1), None),
    );
    let root = component_ref(stack);

    let frame = render_layout_frame(&root, 5, 1);
    assert_eq!(visible_lines(&frame.lines), ["shown"]);
}

#[test]
fn tracks_follow_end_state_and_returns_unused_scroll_delta() {
    let scroll_view = ScrollView::new(
        text("1\n2\n3\n4\n5\n6"),
        ScrollViewOptions {
            follow_end: true,
            primary: true,
            ..ScrollViewOptions::default()
        },
    );
    let state = scroll_view.state();
    let root = component_ref(scroll_view);

    render_layout_frame(&root, 10, 3);
    assert_eq!(state.borrow().scroll_top(), 3);
    assert!(state.borrow().is_following_end());

    assert_eq!(state.borrow_mut().scroll_by(-2), 0);
    assert_eq!(state.borrow().scroll_top(), 1);
    assert!(!state.borrow().is_following_end());
    assert_eq!(state.borrow_mut().scroll_by(-3), -2);
    assert_eq!(state.borrow().scroll_top(), 0);
    assert_eq!(state.borrow_mut().scroll_by(10), 7);
    assert_eq!(state.borrow().scroll_top(), 3);
    assert!(state.borrow().is_following_end());
}

#[test]
fn updates_reserved_scrollbar_layout_at_runtime() {
    let scroll_view = ScrollView::new(
        text("123456"),
        ScrollViewOptions {
            scrollbar: ScrollViewScrollbar::Always,
            ..ScrollViewOptions::default()
        },
    );
    let state = scroll_view.state();
    let scroll_component = component_ref(scroll_view);
    let mut stack = HStack::new(StackOptions {
        gap: None,
        align: Some(notagent_tui::layout_node::StackAlign::Start),
    });
    stack.add_child(scroll_component);
    let root = component_ref(stack);

    let always = render_layout_frame(&root, 6, 2);
    assert_eq!(visible_lines(&always.lines), ["12345", "6"]);
    assert_eq!(always.root.children[0].rect.width, 6);
    assert_eq!(always.root.children[0].children[0].rect.width, 5);

    state
        .borrow_mut()
        .set_scrollbar(ScrollViewScrollbar::Hidden);
    let hidden = render_layout_frame(&root, 6, 2);
    assert_eq!(hidden.root.children[0].children[0].rect.width, 6);
    assert!(!state.borrow().is_scrollbar_visible());
}

#[test]
fn measures_nested_scroll_content_from_constrained_child_geometry() {
    let inner = ScrollView::new(text("1\n2\n3\n4\n5\n6"), ScrollViewOptions::default());
    let inner_state = inner.state();
    let inner_component = component_ref(inner);

    let mut stack = VStack::new(StackOptions::default());
    stack.add_child_with(
        inner_component,
        StackEntryOptions {
            basis: Some(StackBasis::Size(2)),
            ..StackEntryOptions::default()
        },
    );
    stack.add_child(text("tail"));

    let outer = ScrollView::new(component_ref(stack), ScrollViewOptions::default());
    let outer_state = outer.state();
    let root = component_ref(outer);

    render_layout_frame(&root, 10, 2);

    assert_eq!(inner_state.borrow().viewport_height(), 2);
    assert_eq!(outer_state.borrow_mut().scroll_by(10), 9);
    assert_eq!(outer_state.borrow().scroll_top(), 1);
}

#[test]
fn rebuilds_geometry_after_content_changes() {
    let text_component = Rc::new(RefCell::new(Text::new("one", 0, 0)));
    let mut stack = VStack::new(StackOptions::default());
    stack.add_child(text_component.clone() as ComponentRef);
    let root = component_ref(stack);

    let first = render_layout_frame(&root, 10, 4);
    text_component.borrow_mut().set_text("one\ntwo\nthree");
    let second = render_layout_frame(&root, 10, 4);

    assert_eq!(first.root.children[0].lines.as_ref().map(Vec::len), Some(1));
    assert_eq!(
        second.root.children[0].lines.as_ref().map(Vec::len),
        Some(3)
    );
}

/// Thumb height for a given content height (part of the scrollbar case).
fn thumb_height_for(content_height: usize, scrollbar_background: &str) -> usize {
    let scrollbar_style: Rc<dyn Fn(&str) -> String> = {
        let background = scrollbar_background.to_string();
        Rc::new(move |text| format!("{background}{text}\x1b[49m"))
    };
    let content = text(&vec!["x"; content_height].join("\n"));
    let sized = ScrollView::new(
        content,
        ScrollViewOptions {
            scrollbar: ScrollViewScrollbar::Auto,
            scrollbar_style,
            ..ScrollViewOptions::default()
        },
    );
    let state = sized.state();
    let root = component_ref(sized);
    render_layout_frame(&root, 6, 20);
    state.borrow_mut().scroll_by(1);
    render_layout_frame(&root, 6, 20)
        .lines
        .iter()
        .filter(|line| line.contains(scrollbar_background))
        .count()
}

#[test]
fn renders_a_transient_proportional_scrollbar_without_replacing_cell_content() {
    let source_lines = [
        "abcd界", "abcde2", "abcde3", "abcde4", "abcde5", "abcde6", "abcde7", "abcde8",
    ];
    let content_background = "\x1b[42m";
    let scrollbar_background = "\x1b[48;5;1m";
    let scrollbar_style: Rc<dyn Fn(&str) -> String> =
        Rc::new(move |text| format!("\x1b[48;5;1m{text}\x1b[49m"));

    let mut content = Text::new(source_lines.join("\n"), 0, 0);
    content.set_custom_bg_fn(Some(Rc::new(move |text| format!("\x1b[42m{text}\x1b[49m"))));
    let content_component: ComponentRef = Rc::new(RefCell::new(content));

    let scroll_view = ScrollView::new(
        content_component.clone(),
        ScrollViewOptions {
            scrollbar: ScrollViewScrollbar::Auto,
            scrollbar_style: Rc::clone(&scrollbar_style),
            scrollbar_hide_delay_ms: 10,
            ..ScrollViewOptions::default()
        },
    );
    let state = scroll_view.state();
    let root = component_ref(scroll_view);

    let thumb_rows = |lines: &[String]| -> Vec<bool> {
        lines
            .iter()
            .map(|line| line.contains(scrollbar_background))
            .collect()
    };

    let frame = render_layout_frame(&root, 6, 4);
    assert_eq!(thumb_rows(&frame.lines), [false, false, false, false]);
    assert_eq!(
        frame
            .lines
            .iter()
            .map(|line| strip_terminal_sequences(line))
            .collect::<Vec<_>>(),
        source_lines[..4]
    );

    state.borrow_mut().scroll_by(2);
    let frame = render_layout_frame(&root, 6, 4);
    assert_eq!(thumb_rows(&frame.lines), [false, true, true, false]);
    assert_eq!(
        frame
            .lines
            .iter()
            .map(|line| strip_terminal_sequences(line))
            .collect::<Vec<_>>(),
        source_lines[2..6]
    );
    assert!(frame.lines[1].rfind(content_background) < frame.lines[1].rfind(scrollbar_background));

    // The transient scrollbar hides once its deadline passes.
    state.borrow_mut().fire_scrollbar_hide();
    let frame = render_layout_frame(&root, 6, 4);
    assert_eq!(thumb_rows(&frame.lines), [false, false, false, false]);

    state.borrow_mut().scroll_to_end();
    let frame = render_layout_frame(&root, 6, 4);
    assert_eq!(thumb_rows(&frame.lines), [false, false, true, true]);
    assert_eq!(
        frame
            .lines
            .iter()
            .map(|line| strip_terminal_sequences(line))
            .collect::<Vec<_>>(),
        source_lines[4..]
    );

    // Growth while following the end does not show the scrollbar.
    let followed_content = Rc::new(RefCell::new(Text::new(source_lines.join("\n"), 0, 0)));
    let followed = ScrollView::new(
        followed_content.clone() as ComponentRef,
        ScrollViewOptions {
            follow_end: true,
            scrollbar: ScrollViewScrollbar::Auto,
            scrollbar_style: Rc::clone(&scrollbar_style),
            ..ScrollViewOptions::default()
        },
    );
    let followed_state = followed.state();
    let followed_root = component_ref(followed);
    render_layout_frame(&followed_root, 6, 4);
    assert_eq!(followed_state.borrow().scroll_top(), 4);
    followed_content
        .borrow_mut()
        .set_text(format!("{}\nabcde9", source_lines.join("\n")));
    let growth_frame = render_layout_frame(&followed_root, 6, 4);
    assert_eq!(followed_state.borrow().scroll_top(), 5);
    assert!(
        growth_frame
            .lines
            .iter()
            .all(|line| !line.contains(scrollbar_background))
    );

    // Content that fits never shows a scrollbar.
    let fitting_content: ComponentRef = Rc::new(RefCell::new(Text::new("1\n2", 0, 0)));
    let automatic = ScrollView::new(
        fitting_content.clone(),
        ScrollViewOptions {
            scrollbar: ScrollViewScrollbar::Auto,
            scrollbar_style: Rc::clone(&scrollbar_style),
            ..ScrollViewOptions::default()
        },
    );
    let automatic_state = automatic.state();
    let automatic_root = component_ref(automatic);
    render_layout_frame(&automatic_root, 6, 4);
    automatic_state.borrow_mut().scroll_by(1);
    assert!(
        render_layout_frame(&automatic_root, 6, 4)
            .lines
            .iter()
            .all(|line| !line.contains(scrollbar_background))
    );

    // `always` reserves a column even when the content fits.
    let always_fitting = ScrollView::new(
        fitting_content,
        ScrollViewOptions {
            scrollbar: ScrollViewScrollbar::Always,
            scrollbar_style: Rc::clone(&scrollbar_style),
            ..ScrollViewOptions::default()
        },
    );
    let always_fitting_frame = render_layout_frame(&component_ref(always_fitting), 6, 4);
    assert_eq!(always_fitting_frame.root.children[0].rect.width, 5);
    assert!(
        always_fitting_frame
            .lines
            .iter()
            .all(|line| line.contains(scrollbar_background))
    );

    let always_overflowing = ScrollView::new(
        content_component,
        ScrollViewOptions {
            scrollbar: ScrollViewScrollbar::Always,
            scrollbar_style: Rc::clone(&scrollbar_style),
            ..ScrollViewOptions::default()
        },
    );
    let always_overflowing_frame = render_layout_frame(&component_ref(always_overflowing), 6, 4);
    assert_eq!(always_overflowing_frame.root.children[0].rect.width, 5);
    assert_eq!(
        always_overflowing_frame
            .lines
            .iter()
            .filter(|line| line.contains(scrollbar_background))
            .count(),
        2
    );

    assert_eq!(thumb_height_for(21, scrollbar_background), 19);
    assert_eq!(thumb_height_for(40, scrollbar_background), 10);
    assert_eq!(thumb_height_for(100, scrollbar_background), 4);
    assert_eq!(thumb_height_for(400, scrollbar_background), 2);
}

/// Keeps `ScrollViewState` referenced for the doc link above.
type _State = ScrollViewState;
