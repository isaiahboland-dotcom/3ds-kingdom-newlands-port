//! Things the monarch pays coins into. orig: `Payable`, `PayableManager`.

use crate::payment::{PayableId, Target};

/// Max distance for the selected payable to stay selected, and the search
/// range added on top of each payable's own `pay_distance`.
/// orig: `Player.UpdatePayState()` / `PayableManager.GetClosestPayable(x, 0.5f)`.
const SELECT_RANGE: f32 = 0.5;
/// Price clamp after `price_increase`. orig: `Payable.MIN_COIN_PRICE` / `MAX_COIN_PRICE`.
const MIN_PRICE: u8 = 1;
const MAX_PRICE: u8 = 16;
/// orig: `Dejiggler.RoundToPixels` at `ScreenManager.PixelsPerUnit`.
const PIXELS_PER_UNIT: f32 = 32.0;

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq)]
pub struct Payable {
    pub id: PayableId,
    pub x: f32,
    /// Ground-relative y of the payable's origin (coin slots float above it).
    pub y: f32,
    pub price: u8,
    /// Added to the price after every purchase. orig: `Payable.priceIncrease`.
    pub price_increase: u8,
    /// orig: `Payable.CanPay()` (tutorial gating, build state, …).
    pub can_pay: bool,
    /// orig: `Payable.floatingCoinPosition`.
    pub coin_offset: (f32, f32),
    /// orig: `Payable.floatingCoinSpacing`.
    pub coin_spacing: f32,
    /// orig: `Payable.playerPayPointOffset.x`.
    pub pay_point_offset: f32,
    /// orig: `Payable.playerPayDistance`.
    pub pay_distance: f32,
}

impl Payable {
    pub fn new(id: PayableId, x: f32, price: u8) -> Self {
        Self {
            id,
            x,
            y: 0.0,
            price,
            price_increase: 0,
            can_pay: true,
            coin_offset: (0.0, 2.8),
            coin_spacing: 0.4,
            pay_point_offset: 0.0,
            pay_distance: 1.0,
        }
    }

    pub fn target(&self) -> Target {
        Target {
            id: self.id,
            can_pay: self.can_pay,
            price: self.price,
        }
    }

    /// orig: `Payable.PlayerPayPoint()`.
    pub fn pay_point(&self) -> f32 {
        self.x + self.pay_point_offset
    }

    /// World position of price slot `index`. Prices of 7 and up use two rows.
    /// orig: `Payable.FloatingCoinPosition()`.
    pub fn coin_slot(&self, index: u8) -> (f32, f32) {
        let (bx, by) = (self.x + self.coin_offset.0, self.y + self.coin_offset.1);
        let price = self.price;
        let (ox, oy) = if price < 7 {
            self.coin_in_row(index, price)
        } else {
            // orig: Mathf.CeilToInt(price / 2), integer division first, so this floors.
            let first = price / 2;
            if index < first {
                self.coin_in_row(index, first)
            } else {
                let (x, y) = self.coin_in_row(index - first, price - first);
                (x, y + 0.4)
            }
        };
        (bx + ox, by + oy)
    }

    /// orig: `Payable.FloatingCoinInRow()`.
    fn coin_in_row(&self, index: u8, in_row: u8) -> (f32, f32) {
        let width = self.coin_spacing * (in_row as f32 - 1.0);
        let x = -0.5 * width + index as f32 * self.coin_spacing;
        let y = 0.3 * (libm::cosf(x) - 1.0);
        (round_to_pixels(x), round_to_pixels(y))
    }

    /// orig: the tail of `Payable.TransactionComplete()`.
    pub fn apply_price_increase(&mut self) {
        if self.price_increase != 0 {
            self.price = self
                .price
                .saturating_add(self.price_increase)
                .clamp(MIN_PRICE, MAX_PRICE);
        }
    }
}

fn round_to_pixels(v: f32) -> f32 {
    libm::roundf(v * PIXELS_PER_UNIT) / PIXELS_PER_UNIT
}

/// Picks the payable the monarch is paying into this tick. None while galloping.
/// orig: the targeting at the top of `Player.UpdatePayState()`.
pub fn choose_target(
    payables: &[Payable],
    selected: Option<PayableId>,
    x: f32,
    running: bool,
) -> Option<usize> {
    if running {
        return None;
    }
    if let Some(i) = selected.and_then(|id| payables.iter().position(|p| p.id == id)) {
        let p = &payables[i];
        if p.can_pay && (p.x - x).abs() < SELECT_RANGE {
            return Some(i);
        }
    }
    // orig: PayableManager.RetrieveClosestPayable
    payables
        .iter()
        .enumerate()
        .filter(|(_, p)| p.can_pay && (p.pay_point() - x).abs() - p.pay_distance < SELECT_RANGE)
        .min_by(|(_, a), (_, b)| {
            (a.pay_point() - x)
                .abs()
                .total_cmp(&(b.pay_point() - x).abs())
        })
        .map(|(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_row_is_centered() {
        let p = Payable::new(PayableId(0), 10.0, 3);
        let xs: [f32; 3] = core::array::from_fn(|i| p.coin_slot(i as u8).0);
        assert_eq!(xs, [9.59375, 10.0, 10.40625]);
        // The row curves down at the ends.
        assert!(p.coin_slot(0).1 < p.coin_slot(1).1);
    }

    #[test]
    fn large_prices_use_two_rows() {
        let p = Payable::new(PayableId(0), 0.0, 9);
        // 9 / 2 = 4 in the bottom row, 5 in the top row.
        assert!(p.coin_slot(3).1 < 2.81);
        assert!(p.coin_slot(4).1 > 3.0);
    }

    #[test]
    fn targeting() {
        let ps = [
            Payable::new(PayableId(1), 0.0, 1),
            Payable::new(PayableId(2), 2.0, 1),
        ];
        assert_eq!(choose_target(&ps, None, 1.2, false), Some(1));
        assert_eq!(choose_target(&ps, None, 0.8, false), Some(0));
        assert_eq!(choose_target(&ps, None, 0.8, true), None);
        assert_eq!(choose_target(&ps, None, 5.0, false), None);
        // A selected payable within 0.5 wins even if another is closer.
        let close = [
            Payable::new(PayableId(1), 0.0, 1),
            Payable::new(PayableId(2), 0.8, 1),
        ];
        assert_eq!(choose_target(&close, None, 0.45, false), Some(1));
        assert_eq!(
            choose_target(&close, Some(PayableId(1)), 0.45, false),
            Some(0)
        );
    }
}
