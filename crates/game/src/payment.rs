//! Tap-to-drop / hold-to-pay state machine for the pay button.
//!
//! orig: `Player.UpdatePayState()`, `Player.AddCoinToSelectedPayable()`,
//! `Player.CancelTransaction()`, `Player.TryDropCoin()`.
//!
//! The caller chooses the target payable each tick (orig: the selected payable
//! if it can still be paid and is within 0.5 units, otherwise
//! `PayableManager.GetClosestPayable(x, 0.5)`, and none while galloping) and
//! applies the emitted [`PayEvent`]s to the world.

/// Timing for the pay button. All values are from the Player prefab (scene).
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PayTiming {
    /// A press shorter than this is a tap. orig: `Player.keyDownThreshold`.
    pub hold_threshold: f32,
    /// orig: `Player.timeBetweenCoins`.
    pub between_coins: f32,
    /// Delay after the last coin before the purchase happens. orig: `Player.timeBeforeTransaction`.
    pub before_transaction: f32,
    /// After releasing mid-payment, floating coins drop once this has passed
    /// since the last coin. orig: `Player.timeBeforeCancel`.
    pub before_cancel: f32,
}

impl Default for PayTiming {
    fn default() -> Self {
        Self {
            hold_threshold: 0.15,
            between_coins: 0.3,
            before_transaction: 0.3,
            before_cancel: 0.7,
        }
    }
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PayableId(pub u32);

/// The payable the monarch is standing at this tick.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    pub id: PayableId,
    /// orig: `Payable.CanPay()`.
    pub can_pay: bool,
    /// orig: `Payable.price`.
    pub price: u8,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, Default)]
pub struct PayInput {
    pub dt: f32,
    /// Pay button is held.
    pub key: bool,
    /// Pay button went down this tick.
    pub key_down: bool,
    pub target: Option<Target>,
    /// Coins currently in the wallet.
    pub coins: u32,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PayEvent {
    /// The monarch is now standing at this payable. orig: `Player.SelectPayable()`.
    Focus(PayableId),
    /// orig: `Player.DeselectPayable()`.
    Unfocus(PayableId),
    /// Drop one coin from the wallet onto the ground. orig: `Wallet.DropCoin()`.
    DropCoin,
    /// Show the price indicators. orig: `Payable.Select()`.
    ShowPrice(PayableId),
    /// Move one coin from the wallet into price slot `slot`. orig: `Wallet.MoveCoin()`.
    CoinToPayable { id: PayableId, slot: u8 },
    /// Every slot is filled. orig: `Payable.Highlight()`.
    Highlight(PayableId),
    /// Purchase happens. The floating coins are spent. orig: `Payable.TransactionComplete()`.
    Complete { id: PayableId, coins: u8 },
    /// Payment abandoned. The floating coins fall as real coins only the
    /// monarch can pick up. orig: the `PayState.Cancelling` branch.
    ReleaseCoins { count: u8 },
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PayState {
    None,
    Holding,
    Transaction,
    Cancelling,
    Completed,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug)]
pub struct PayMachine {
    pub timing: PayTiming,
    /// orig: `Player.coinDroppingEnabled`.
    pub coin_dropping_enabled: bool,
    state: PayState,
    timer: f32,
    selected: Option<Target>,
    completing: Option<PayableId>,
    floating: u8,
}

impl Default for PayMachine {
    fn default() -> Self {
        Self::new(PayTiming::default())
    }
}

impl PayMachine {
    pub fn new(timing: PayTiming) -> Self {
        Self {
            timing,
            coin_dropping_enabled: true,
            state: PayState::None,
            timer: 0.0,
            selected: None,
            completing: None,
            floating: 0,
        }
    }

    pub fn state(&self) -> PayState {
        self.state
    }

    /// Coins currently floating above the payable.
    pub fn floating(&self) -> u8 {
        self.floating
    }

    /// The payable the monarch is standing at. orig: `Player.selectedPayable`.
    pub fn selected(&self) -> Option<PayableId> {
        self.selected.map(|t| t.id)
    }

    fn set_state(&mut self, state: PayState) {
        self.timer = 0.0;
        self.state = state;
    }

    /// orig: `Player.CancelTransaction()`.
    pub fn cancel(&mut self) {
        match self.state {
            PayState::Holding => self.set_state(PayState::None),
            PayState::Transaction => self.set_state(PayState::Cancelling),
            _ => {}
        }
    }

    /// orig: `Player.UpdatePayState()`.
    pub fn update(&mut self, input: &PayInput, mut emit: impl FnMut(PayEvent)) {
        let target_id = input.target.map(|t| t.id);
        if target_id != self.selected.map(|t| t.id) {
            self.cancel();
            if let Some(old) = self.selected {
                emit(PayEvent::Unfocus(old.id));
            }
            if let Some(id) = target_id {
                emit(PayEvent::Focus(id));
            }
        }
        // Price and can_pay can change while focused (e.g. after an upgrade).
        self.selected = input.target;

        match self.state {
            PayState::None => {
                if input.key_down && input.coins > 0 {
                    if self.selected.is_some_and(|t| t.can_pay) {
                        self.set_state(PayState::Holding);
                    } else if self.coin_dropping_enabled {
                        emit(PayEvent::DropCoin);
                    }
                }
            }
            PayState::Holding => {
                if !input.key {
                    // Tap at a payable: drop a coin on release. orig: `TryDropCoin()`.
                    if self.coin_dropping_enabled {
                        emit(PayEvent::DropCoin);
                    }
                    self.set_state(PayState::None);
                    return;
                }
                self.timer += input.dt;
                if self.timer > self.timing.hold_threshold {
                    self.set_state(PayState::Transaction);
                    if let Some(t) = self.selected {
                        emit(PayEvent::ShowPrice(t.id));
                    }
                }
            }
            PayState::Transaction => {
                self.timer += input.dt;
                let Some(target) = self.selected else {
                    self.cancel();
                    return;
                };
                if !input.key && self.timer > self.timing.before_cancel {
                    self.cancel();
                } else if input.key
                    && (self.timer > self.timing.between_coins || self.floating == 0)
                {
                    if input.coins == 0 {
                        self.cancel();
                    } else {
                        self.add_coin(target, &mut emit);
                    }
                }
            }
            PayState::Cancelling => {
                if self.floating > 0 {
                    emit(PayEvent::ReleaseCoins {
                        count: self.floating,
                    });
                }
                self.floating = 0;
                self.set_state(PayState::None);
            }
            PayState::Completed => {
                self.timer += input.dt;
                if self.timer <= self.timing.before_transaction {
                    return;
                }
                if let Some(id) = self.completing.take() {
                    emit(PayEvent::Complete {
                        id,
                        coins: self.floating,
                    });
                }
                self.floating = 0;
                self.set_state(PayState::None);
            }
        }
    }

    /// orig: `Player.AddCoinToSelectedPayable()`.
    fn add_coin(&mut self, target: Target, emit: &mut impl FnMut(PayEvent)) {
        self.timer = 0.0;
        emit(PayEvent::CoinToPayable {
            id: target.id,
            slot: self.floating,
        });
        self.floating += 1;
        if self.floating >= target.price {
            self.completing = Some(target.id);
            emit(PayEvent::Highlight(target.id));
            self.set_state(PayState::Completed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    const DT: f32 = 1.0 / 60.0;
    const SHOP: PayableId = PayableId(7);

    fn shop(price: u8) -> Option<Target> {
        Some(Target {
            id: SHOP,
            can_pay: true,
            price,
        })
    }

    /// Simulates the player and the wallet frame by frame.
    struct Sim {
        m: PayMachine,
        coins: u32,
        held: bool,
        target: Option<Target>,
        log: Vec<(u32, PayEvent)>,
        frame: u32,
    }

    impl Sim {
        fn new(coins: u32, target: Option<Target>) -> Self {
            Self {
                m: PayMachine::default(),
                coins,
                held: false,
                target,
                log: Vec::new(),
                frame: 0,
            }
        }

        fn step(&mut self, key: bool) {
            let input = PayInput {
                dt: DT,
                key,
                key_down: key && !self.held,
                target: self.target,
                coins: self.coins,
            };
            self.held = key;
            let mut out = Vec::new();
            self.m.update(&input, |e| out.push(e));
            for e in out {
                if matches!(e, PayEvent::DropCoin | PayEvent::CoinToPayable { .. }) {
                    self.coins -= 1;
                }
                self.log.push((self.frame, e));
            }
            self.frame += 1;
        }

        fn run(&mut self, key: bool, frames: u32) {
            for _ in 0..frames {
                self.step(key);
            }
        }

        fn events(&self) -> Vec<PayEvent> {
            self.log
                .iter()
                .map(|&(_, e)| e)
                .filter(|e| !matches!(e, PayEvent::Focus(_) | PayEvent::Unfocus(_)))
                .collect()
        }
    }

    #[test]
    fn press_away_from_payable_drops_one_coin_on_press() {
        let mut s = Sim::new(10, None);
        s.run(true, 120);
        s.run(false, 5);
        assert_eq!(s.events(), [PayEvent::DropCoin]);
        assert_eq!(s.log[0].0, 0);
        assert_eq!(s.coins, 9);
    }

    #[test]
    fn tap_at_payable_drops_coin_on_release() {
        let mut s = Sim::new(10, shop(3));
        s.run(true, 5); // ~83 ms, under the 150 ms threshold
        s.run(false, 1);
        assert_eq!(s.events(), [PayEvent::DropCoin]);
        assert_eq!(s.m.state(), PayState::None);
    }

    #[test]
    fn hold_pays_one_coin_every_300ms_then_completes() {
        let mut s = Sim::new(10, shop(3));
        s.run(true, 120);
        let ev = s.events();
        assert_eq!(
            ev,
            [
                PayEvent::ShowPrice(SHOP),
                PayEvent::CoinToPayable { id: SHOP, slot: 0 },
                PayEvent::CoinToPayable { id: SHOP, slot: 1 },
                PayEvent::CoinToPayable { id: SHOP, slot: 2 },
                PayEvent::Highlight(SHOP),
                PayEvent::Complete { id: SHOP, coins: 3 },
            ]
        );
        assert_eq!(s.coins, 7);

        let frames: Vec<u32> = s
            .log
            .iter()
            .filter(|(_, e)| matches!(e, PayEvent::CoinToPayable { .. }))
            .map(|&(f, _)| f)
            .collect();
        // Frame 0 enters Holding; 10 ticks later the timer passes 0.15 s, and the
        // first coin goes in the frame after. Coins are then 18 frames apart,
        // because 18 f32 ticks of 1/60 already sum to just over 0.3.
        assert_eq!(frames, [11, 29, 47]);
    }

    #[test]
    fn releasing_mid_payment_releases_coins_after_grace_period() {
        let mut s = Sim::new(10, shop(5));
        s.run(true, 35); // two coins in
        assert_eq!(s.m.floating(), 2);
        s.run(false, 30); // 0.5 s: still inside the 0.7 s grace
        assert_eq!(s.m.state(), PayState::Transaction);
        s.run(false, 30);
        assert_eq!(s.m.state(), PayState::None);
        assert_eq!(
            s.events().last(),
            Some(&PayEvent::ReleaseCoins { count: 2 })
        );
        assert_eq!(s.coins, 8); // the released coins are on the ground, not in the wallet
    }

    #[test]
    fn re_pressing_within_grace_resumes_payment() {
        let mut s = Sim::new(10, shop(3));
        s.run(true, 35);
        s.run(false, 10);
        s.run(true, 120);
        assert!(s
            .events()
            .contains(&PayEvent::Complete { id: SHOP, coins: 3 }));
    }

    #[test]
    fn walking_away_cancels() {
        let mut s = Sim::new(10, shop(5));
        s.run(true, 35);
        s.target = None;
        s.run(true, 2);
        assert_eq!(
            s.events().last(),
            Some(&PayEvent::ReleaseCoins { count: 2 })
        );
        assert!(s.log.iter().any(|&(_, e)| e == PayEvent::Unfocus(SHOP)));
    }

    #[test]
    fn running_out_of_coins_cancels() {
        let mut s = Sim::new(2, shop(5));
        s.run(true, 120);
        assert_eq!(
            s.events().last(),
            Some(&PayEvent::ReleaseCoins { count: 2 })
        );
        assert_eq!(s.coins, 0);
    }

    #[test]
    fn empty_wallet_does_nothing() {
        let mut s = Sim::new(0, shop(1));
        s.run(true, 60);
        assert!(s.events().is_empty());
    }

    #[test]
    fn locked_payable_drops_coin_instead() {
        let mut s = Sim::new(
            10,
            Some(Target {
                id: SHOP,
                can_pay: false,
                price: 1,
            }),
        );
        s.run(true, 60);
        assert_eq!(s.events(), [PayEvent::DropCoin]);
    }
}
