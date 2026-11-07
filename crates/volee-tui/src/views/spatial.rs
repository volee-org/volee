//! Moving through a picture with the arrows: the nearest target in a
//! direction, shared by the graph views.

use ratatui::layout::Rect;
use volee_core::AgentId;

use super::hierarchy::Direction;

/// The nearest target entirely beyond `from` in a direction, among those
/// `allowed`, preferring targets aligned with it. Ties go to the target
/// whose middle is closest to `from`'s, then to the first in `targets`.
pub(crate) fn nearest(
    targets: &[(AgentId, Rect)],
    from: AgentId,
    direction: Direction,
    allowed: impl Fn(AgentId) -> bool,
) -> Option<AgentId> {
    let origin = targets.iter().find(|(id, _)| *id == from)?.1;
    // Distance between two intervals, zero when they overlap.
    let gap = |a0: u16, a1: u16, b0: u16, b1: u16| -> u32 {
        u32::from(b0.saturating_sub(a1).max(a0.saturating_sub(b1)))
    };
    // Twice the center, to stay in integers.
    let center = |start: u16, length: u16| i32::from(start) * 2 + i32::from(length);
    targets
        .iter()
        .filter(|(id, _)| *id != from && allowed(*id))
        .filter_map(|(id, rect)| {
            let (distance, misalignment, offset) = match direction {
                Direction::Down if rect.y >= origin.bottom() => (
                    u32::from(rect.y - origin.bottom()),
                    gap(origin.x, origin.right(), rect.x, rect.right()),
                    center(rect.x, rect.width) - center(origin.x, origin.width),
                ),
                Direction::Up if rect.bottom() <= origin.y => (
                    u32::from(origin.y - rect.bottom()),
                    gap(origin.x, origin.right(), rect.x, rect.right()),
                    center(rect.x, rect.width) - center(origin.x, origin.width),
                ),
                Direction::Right if rect.x >= origin.right() => (
                    u32::from(rect.x - origin.right()),
                    gap(origin.y, origin.bottom(), rect.y, rect.bottom()),
                    center(rect.y, rect.height) - center(origin.y, origin.height),
                ),
                Direction::Left if rect.right() <= origin.x => (
                    u32::from(origin.x - rect.right()),
                    gap(origin.y, origin.bottom(), rect.y, rect.bottom()),
                    center(rect.y, rect.height) - center(origin.y, origin.height),
                ),
                _ => return None,
            };
            Some(((distance + 2 * misalignment, offset.unsigned_abs()), *id))
        })
        .min_by_key(|(score, _)| *score)
        .map(|(_, id)| id)
}

/// The target at the top left, or at the bottom right when `last`.
pub(crate) fn corner(targets: &[(AgentId, Rect)], last: bool) -> Option<AgentId> {
    let key = |rect: &Rect| (rect.y, rect.x);
    let iter = targets.iter();
    if last {
        iter.max_by_key(|(_, rect)| key(rect)).map(|(id, _)| *id)
    } else {
        iter.min_by_key(|(_, rect)| key(rect)).map(|(id, _)| *id)
    }
}

#[cfg(test)]
mod tests {
    use ulid::Ulid;

    use super::*;

    fn id(n: u128) -> AgentId {
        AgentId::from_ulid(Ulid::from_parts(1, n))
    }

    #[test]
    fn the_nearest_aligned_target_wins() {
        // a b
        //   c      d
        let targets = [
            (id(1), Rect::new(0, 0, 4, 1)),
            (id(2), Rect::new(6, 0, 4, 1)),
            (id(3), Rect::new(6, 2, 4, 1)),
            (id(4), Rect::new(20, 2, 4, 1)),
        ];
        let all = |_| true;
        assert_eq!(nearest(&targets, id(1), Direction::Right, all), Some(id(2)));
        assert_eq!(nearest(&targets, id(2), Direction::Down, all), Some(id(3)));
        assert_eq!(nearest(&targets, id(3), Direction::Right, all), Some(id(4)));
        assert_eq!(nearest(&targets, id(3), Direction::Up, all), Some(id(2)));
        assert_eq!(nearest(&targets, id(1), Direction::Left, all), None);
        assert_eq!(
            nearest(&targets, id(1), Direction::Right, |a| a != id(2)),
            Some(id(3))
        );
        assert_eq!(corner(&targets, false), Some(id(1)));
        assert_eq!(corner(&targets, true), Some(id(4)));
    }
}
