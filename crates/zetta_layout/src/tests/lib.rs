use super::*;

#[derive(Clone, Debug, PartialEq)]
enum Tree {
    Pane(u64),
    Split(SplitAxis, u16, Box<Tree>, Box<Tree>),
}

impl LayoutTree for Tree {
    fn node(&self) -> Node<'_, Self> {
        match self {
            Self::Pane(id) => Node::Pane(*id),
            Self::Split(axis, first_ratio, first, second) => Node::Split {
                axis: *axis,
                first_ratio: *first_ratio,
                first,
                second,
            },
        }
    }

    fn node_mut(&mut self) -> NodeMut<'_, Self> {
        match self {
            Self::Pane(id) => NodeMut::Pane(id),
            Self::Split(axis, first_ratio, first, second) => NodeMut::Split {
                axis,
                first_ratio,
                first,
                second,
            },
        }
    }
}

fn split(axis: SplitAxis, first_ratio: u16, first: Tree, second: Tree) -> Tree {
    Tree::Split(axis, first_ratio, Box::new(first), Box::new(second))
}

fn side_by_side(first: Tree, second: Tree) -> Tree {
    split(SplitAxis::Vertical, 500, first, second)
}

fn stacked(first: Tree, second: Tree) -> Tree {
    split(SplitAxis::Horizontal, 500, first, second)
}

fn ids(tree: &Tree) -> Vec<u64> {
    let mut ids = Vec::new();
    collect_pane_ids(tree, &mut ids);
    ids
}

#[test]
fn the_axis_is_carried_as_the_word_the_protocol_always_used() {
    assert_eq!(
        serde_json::to_string(&SplitAxis::Horizontal).unwrap(),
        "\"horizontal\""
    );
    assert_eq!(
        serde_json::from_str::<SplitAxis>("\"vertical\"").unwrap(),
        SplitAxis::Vertical
    );
    assert!(serde_json::from_str::<SplitAxis>("\"diagonal\"").is_err());
}

#[test]
fn panes_are_found_and_listed_in_reading_order() {
    let tree = side_by_side(Tree::Pane(1), stacked(Tree::Pane(2), Tree::Pane(3)));
    assert!(contains_pane(&tree, 3));
    assert!(!contains_pane(&tree, 4));
    assert_eq!(ids(&tree), [1, 2, 3]);
}

#[test]
fn swapping_exchanges_two_panes_across_splits() {
    let mut tree = side_by_side(Tree::Pane(1), stacked(Tree::Pane(2), Tree::Pane(3)));
    swap_pane_ids(&mut tree, 1, 3);
    assert_eq!(ids(&tree), [3, 2, 1]);
}

#[test]
fn moving_swaps_with_the_sibling_on_the_matching_axis() {
    let mut tree = split(SplitAxis::Vertical, 300, Tree::Pane(1), Tree::Pane(2));
    assert!(move_pane(&mut tree, 1, PaneDirection::Right));
    assert_eq!(
        tree,
        split(SplitAxis::Vertical, 700, Tree::Pane(2), Tree::Pane(1))
    );
}

#[test]
fn moving_climbs_to_the_nearest_split_on_the_matching_axis() {
    let mut tree = side_by_side(Tree::Pane(1), stacked(Tree::Pane(2), Tree::Pane(3)));
    assert!(move_pane(&mut tree, 3, PaneDirection::Left));
    assert_eq!(ids(&tree), [2, 3, 1]);
}

#[test]
fn moving_past_the_edge_of_the_layout_changes_nothing() {
    let mut tree = side_by_side(Tree::Pane(1), Tree::Pane(2));
    let before = tree.clone();
    assert!(!move_pane(&mut tree, 1, PaneDirection::Left));
    assert!(!move_pane(&mut tree, 1, PaneDirection::Up));
    assert!(!move_pane(&mut tree, 9, PaneDirection::Right));
    assert_eq!(tree, before);
}

#[test]
fn rotating_two_panes_turns_the_whole_split() {
    let mut tree = side_by_side(Tree::Pane(1), Tree::Pane(2));
    assert!(rotate_pane(&mut tree, 1, PaneRotationDirection::Clockwise));
    assert_eq!(tree, stacked(Tree::Pane(1), Tree::Pane(2)));
    assert!(rotate_pane(&mut tree, 1, PaneRotationDirection::Clockwise));
    assert_eq!(tree, side_by_side(Tree::Pane(2), Tree::Pane(1)));
}

#[test]
fn a_clockwise_and_a_counter_clockwise_turn_cancel_out() {
    let original = split(
        SplitAxis::Vertical,
        600,
        Tree::Pane(1),
        stacked(Tree::Pane(2), Tree::Pane(3)),
    );
    let mut tree = original.clone();
    assert!(rotate_pane(&mut tree, 1, PaneRotationDirection::Clockwise));
    assert!(rotate_pane(
        &mut tree,
        1,
        PaneRotationDirection::CounterClockwise
    ));
    assert_eq!(tree, original);
}

#[test]
fn a_large_pane_carries_the_whole_layout_round() {
    let mut tree = side_by_side(Tree::Pane(1), stacked(Tree::Pane(2), Tree::Pane(3)));
    assert!(rotate_pane(&mut tree, 1, PaneRotationDirection::Clockwise));
    assert_eq!(
        tree,
        stacked(Tree::Pane(1), side_by_side(Tree::Pane(3), Tree::Pane(2)))
    );
}

#[test]
fn a_small_pane_turns_only_its_own_split() {
    let mut tree = side_by_side(Tree::Pane(1), stacked(Tree::Pane(2), Tree::Pane(3)));
    assert!(rotate_pane(&mut tree, 2, PaneRotationDirection::Clockwise));
    assert_eq!(
        tree,
        side_by_side(Tree::Pane(1), side_by_side(Tree::Pane(3), Tree::Pane(2)))
    );
}

#[test]
fn a_pane_outside_the_layout_does_not_rotate_it() {
    let mut tree = side_by_side(Tree::Pane(1), Tree::Pane(2));
    assert!(!rotate_pane(&mut tree, 9, PaneRotationDirection::Clockwise));
    assert_eq!(tree, side_by_side(Tree::Pane(1), Tree::Pane(2)));
}
