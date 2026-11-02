# Systems map

Maps the original C# classes (`reference/decomp/main/`, decompiled from `Assembly-CSharp.dll`) to the Rust modules we plan to write. Values marked **(scene)** were read from the serialized scene/prefab data in the user's `Kingdom_Data/`, not from code defaults. Unity's code defaults are often overridden there, so always prefer the scene value.

Engine: Unity **2017.1.2p1**, Mono. The game logic lives in `Assembly-CSharp.dll`. `Assembly-CSharp-firstpass.dll` holds only plugin code (Rewired, DOTween, etc.).

---

## Architecture of the original

- **Managers.** `Managers` is a static locator (`Managers.director`, `Managers.kingdom`, `Managers.enemies`, …). Each manager derives from `Manager` and has `Init()` and `OnLevelLoaded()` hooks.
- **Persistence.** Persistent components implement `IPersistent` (`RetrieveData()` / `ApplyData()`) and pair with a plain `*Data` class (e.g. `Director` ↔ `DirectorData`). `PersistentManager` collects them. This maps to our save format.
- **Payables.** Anything the monarch pays for derives from `Payable`, which has `price`, `Pay()` and `CanPay()`. `PayableManager.GetClosestPayable(x, 0.5)` picks the target.
- **Workables.** Jobs that builders perform (walls, trees, boat, catapult) derive from `Workable`. `JobAssigner` / `Kingdom` hand them out.
- **Data assets.** `Day`, `Wave`, `LevelConfig`, `BlenderState`, `EnemyData`… are ScriptableObjects. The extract tool needs to export them.
- **Procedural levels.** Islands are **generated**, not hand-placed. `Level` + `LevelLayout` lay out `LevelBlock` prefabs in `LevelBlockGroup`s, with per-island counts from `LevelConfig.groupCounts` and a minimum width of `minLevelWidth = 400`. So the asset pipeline needs the block prefabs and configs, not a fixed map.

---

## Day/night cycle — `Director`, `Kingdom`, `Day`, `CelestialBody`

**Clock** (`Director.Update`): `currentTime += dt * clockSpeedModifier / secondsPerInGameHour`. When the time reaches 24, it wraps and `currentDay++`. The clock is fully **linear**: no phase runs faster or slower than another.

| Value | Source | Value |
|---|---|---|
| `secondsPerInGameHour` | Director **(scene)** | **10 s** → one 24 h day = **240 s (4 min)** real time |
| `clockSpeedModifier` | Director **(scene)** | 1.0 (only debug tools change it) |
| `startTime` | Director **(scene)** | 04:00 on day 0 |

**Gameplay day vs. night** (`Kingdom.Update`) decides the day counter, safe/unsafe state and `OnDayStart`/`OnDayEnd`:

| | Source | Value |
|---|---|---|
| `Kingdom.dayStart` | **(scene)** | **05:00** |
| `Kingdom.dayEnd` | **(scene)** | **21:30** |
| Daytime | | 05:00–21:30 = 16.5 h = **165 s** |
| Night | | 21:30–05:00 = 7.5 h = **75 s** |

**Displayed day number:** when daytime starts, `day = currentDay + 1`, and `InterfaceOverlay.ShowDayCounter` runs if `currentDay > 0`. During the night, `day` stays at its previous value.

**Visual transitions** (`Director`, all **(scene)**). Each one pushes a `BlenderState` (lighting/colour preset chosen by weather × season × part of day) and blends over the given window:

| Part of day | Start | End |
|---|---|---|
| Dawn | 05:00 | 07:30 |
| Day | 08:00 | 13:00 |
| Evening | 16:00 | 19:00 |
| Night | 21:15 | 23:30 |

**Waves:** `waveAtCastle` = **01:00** **(scene)**. `ScheduleWaveToday` works backward from the arrival time, using `EnemyManager.GetWaveTravelTime`, to find when greed must spawn at the portal. The portal opens 6 in-game hours before that.

**Day schedule:** `Director.AddCycle` queues 3–4 random `regularDays` (22 available), then 1 of 4 `bossDays`, then 1 of 4 `recoveryDays`. Each `Day` picks a random weather and its music tracks. Seasons: autumn and winter start on days drawn from `LevelConfig.autumnStart` / `winterStart`, and `Season` is a flags enum (Summer=1, Autumn=2, Winter=4). `LevelConfig.daysBeforeGameOver` sets an optional day limit.

**Sun/moon:** `CelestialBody` moves along a half-ellipse between `risesAt` and `setsAt` using `CircularInterval`.

**Implication for the bottom-screen daylight timer:** because the clock is linear, a simple bar works. Show the sun from 05:00 to 21:30 (165 s) and the moon from 21:30 to 05:00 (75 s). We don't need the phase-icon fallback.

→ Rust: `game::time` (clock, day counter, part-of-day), `game::director` (event queue, day/wave scheduling).

---

## Coins and payment — `Player` (pay state machine), `Wallet`, `Payable`, `Coin`, `CoinBag`

The original **already has tap vs. hold**. In `Player.UpdatePayState`, the states are `None → Holding → Transaction → Completed/Cancelling`.

| Setting | Source | Value |
|---|---|---|
| `keyDownThreshold` (tap vs. hold) | Player prefab **(scene)** | **0.15 s** |
| `timeBetweenCoins` (pay rate) | **(scene)** | **0.30 s** → ~3.3 coins/s |
| `timeBeforeTransaction` (after the last coin, until `TransactionComplete`) | **(scene)** | 0.30 s |
| `timeBeforeCancel` (release mid-payment; floating coins return after this) | **(scene)** | 0.70 s |
| `overflowCoinsWaterProbability` | **(scene)** | 0.5 |

How the original behaves:
- Pressing near a payable enters `Holding`.
- Releasing before 0.15 s drops one coin (`TryDropCoin`).
- Pressing away from any payable drops a coin **immediately on press**.
- Holding past 0.15 s selects the payable. The first coin goes in immediately, then one coin every 0.3 s.
- Releasing for more than 0.7 s mid-payment cancels, and the floating coins fall back to the ground.
- Payable targeting only works while not galloping (`ActionState.Run` disables it), within 0.5 units.

→ Rust: `game::economy` (wallet, coins, gems), `game::payment` (state machine, a direct port).

---

## Monarch and steed — `Player.UpdateActionState`, `Steed`, `Mover`

Ported in `game::monarch`. The actions are Stand / Walk / Run (gallop) / Eat / Rear.
- **Stamina.** Galloping drains stamina at `runStaminaRate`. At 0 there's a `reserveProbability` chance of a `reserveStamina` refill; otherwise the steed tires for `tiredDuration`. Trying to gallop while tired makes it rear instead.
- **Eating.** Standing on grass for longer than `eatDelay` starts eating (`eatDuration`), which refills stamina and starts the `wellFedDuration` timer.
- **Forests.** They multiply speed by `forestSpeedMultiplier`.
- **Sprint input.** A double-tap of a direction within **0.6 s** (Rewired `_buttonDoublePressSpeed`, loader scene) or the sprint button starts a gallop. Releasing the sprint button drops back to a walk.
- **Payable targeting** is disabled while galloping.

Default steed is **Horse Regular** (`Player.standardSteedPrefab`): walk 2.0, gallop 3.6, forest ×0.85, gallop drain −0.08/s (≈12.5 s from full), tired 20 s, reserve 0.25 @ 70 %, well-fed 48 s. Other steeds' stats are in `Resources/prefabs/steeds/*.prefab`.

## Coins — `Wallet`, `Coin`, `Droppable`

Ported in `game::coins`:
- **Drops.** A dropped coin leaves the purse at (0.125 × facing, 0.79), with a random sideways speed in ±0.5 and an upward speed of 2.5.
- **Pickup policies.** A coin the monarch drops can't be picked back up by them until it leaves their grab box, or for up to 5 s. Then anyone can take it, and it fades out after 120 s.
- **Payments.** Coins spent on a payable fly to price slots at 5 units/s. If the payment is cancelled, they fall as ordinary coins that never expire.
- **Limits.** The wallet holds at most 1000 coins.

## Islands — `Level`, `LevelLayout`, `LevelBlock`, `World`

Ported in `game::level` (generation) and `game::build` (forests). Each island is assembled from the 37 blocks in `blocks.unity`:
- **Choosing blocks.** Required blocks are placed first: those whose groups are under their `LevelConfig.groupCounts` minimum, highest need first. Then random placeable blocks are added until `minLevelWidth` is reached.
- **Ordering.** Each block gets a random normalised position within its min/max distance, on its side. Blocks are sorted by that position and laid end to end, with the `absoluteCenter` block (the starting area) centred on x = 0.
- **Scattering.** Trees, shrubs, rocks, hills, backdrops, etc. (19 kinds) are scattered over contiguous runs of tiles that have the matching `Tile.scatter*` flag.
- **Parallax correction and randomizers.** Then parallax correction (x × layer ratio) and the randomizers (`RandomizeSprite`, `RandomizeTree`).
- **Forests.** These are merged `ForestItem` intervals. They're NotBuildable and slow the steed (`forestSpeedMultiplier`).

`extract` turns the scenes into `game::data::WorldData` (`world.bin`, postcard), which both backends load.

## Build sites — `PayableUpgrade`, `WorkableBuilding`, `Scaffolding`, `Castle`

Ported in `game::build`. Paying replaces the object with `nextPrefab`:
- **Walls:** Wall0 (1) → Wall1 (3) → Wall2 (6, stone) → Wall3 (9, stone) → Wall4.
- **Towers:** Tower0 (3) → Tower1 (5) → Tower2 (7, stone) → Tower3 (9, stone) → Tower4.
- **Farmhouses:** Farmhouse0 (3) → Farmhouse1 (8) → Farmhouse2 (4).
- **Castle:** campfire (3) → Castle1 (3) → Castle2 (6) → Castle3 (7) → Castle4 (8, stone) → Castle5 (9, stone) → Castle6.

`CanPay` requires all of these:
- a next stage exists;
- the site isn't on cooldown or under construction;
- stone tech, if that stage needs it;
- the site isn't inside a forest.

Stages with a `WorkableBuilding` start as their scaffold, and modules fill nearest-first as build points accumulate. Castle levels swap in the dormant shops: level 1 bow and hammer, level 3 scythe, level 4 workshop.

Builders do the work (see Citizens). Castle-level effects and hermit (passenger) upgrades are under "Castle levels" and "Hermits" below.

## Citizens — `Character`, `Beggar`, `BeggarCamp`, `Peasant`, `Worker`, `Archer`, `PayableShop`

Ported in `game::units`, with work assignment in `game::kingdom`. Each original coroutine state is an explicit enum with timers. Transitions are re-checked once a second while a state runs, as in `StateMachine.Update`.

- **Beggars** (walk 0.4, run 1.4):
  - Wander, and after 50 s join the nearest camp, idling within ±4 of it.
  - Every ~0.67 s they look for a coin the monarch dropped within 4 units, claim it for 5 s and run to it.
  - Picking it up makes them a peasant (wallet: 1 coin, monarch's coins only).
  - Camps refill to `maxBeggars`, spawning one per `spawnInterval`.
- **Peasants** (walk 0.6, run 1.4): walk to a random shop and wait 8–19 s. Any tool within 10 units, including tools on a shop rack, draws them over, and touching it promotes them: bow → archer, hammer → worker, scythe → farmer.
- **Shops**:
  - Paying puts a tool in the next free rack slot (`itemPlacement` + slot × `itemSpacing`, up to `maxItems`).
  - `CanPay` needs room on the rack.
  - New shops are scaffolds that build themselves (4 points at 1 per second).
- **Workers** (walk 0.65, run 1.4; strokes of 0.7 s):
  - Idle shuffles alternate with sheltering inside the borders, running at night if they're far out.
  - Jobs: scaffolds with no auto-build (2 workers each) and marked trees (1 worker).
  - Every second free workers are matched to jobs by minimum total cost: distance minus 10 per extra worker still needed, solved with the Hungarian algorithm (`JobAssigner`).
  - Each stroke adds a build point or a chop. A tree needs `strength` (8) chops, then drops 1–2 coins and is removed, which shrinks the forest.
- **Trees**: paying 1 coin marks a tree within 4 units of the forest edge, which shows its chop mark.
- **Archers** (walk 0.65, run 1.6): pick a side (random within 10 of the campfire).
  - **By day (safe):** roam from −8 to +16 beyond that side's border and wait 4–10 s.
  - **At night (unsafe):** stand 1–2 units inside the outer wall.
- **Taxes**: a citizen with more than 1 coin touching the monarch (not galloping) pauses, then tosses the extra coins toward the monarch, one every 0.1 s after 0.4 s. The coins are claimed for the monarch.
- **Borders**: the outermost wall on each side of the campfire, or campfire ± 4.

Since ported (sections below): archer hunting, fleeing, stun and demotion, farming, bakers, knights and tower guard slots.

**Not yet ported:**
- beggars ducking from greed;
- abandoned-tool claims.

## Greed — `Director.ScheduleWaveToday`, `EnemyManager`, `Portal`, `Troll`, `Archer.Shoot`, `Arrow`, `Crown`

Ported in `game::greed`; damage is applied in `game::state`.
- **Waves.**
  - Each scheduled day carries a wave: regular, boss or recovery, by season, from `LevelConfig`.
  - Its size is `floor(curve(day) × multiplier)` for each enemy type, ×`waveTimedMultiplier` (0.5) during the first days (`waveMultiplierTimeframe`).
  - Side: one side at random unless the wave is `spawnOnBothSides`. It leaves from the portal nearest the campfire on that side.
  - Timing: the spawn is timed so the slowest member (troll walk 2) reaches the border at `waveAtCastle` (01:00 the next night). The portal opens 6 hours earlier.
  - Spawning: trolls leave the portal every min(1, 15/count) s.
- **Trolls** (walk 2, run 2.8 for 12 s after a charge, 1 HP; tough trolls have 5).
  - **Chasing:** they walk at the monarch, or at random once the crown is gone. Every 3 s a troll that is too far ahead of its wave's middle slows to 30%, and after their first 5 s they sometimes hop.
  - **Charging:** they leap (8, 2.3) at targets within 2 units. Walls ahead and archers come first; then loot within 3 units; then other citizens and the monarch.
  - **Hitting:** a hit does 1 damage. Walls bounce them back.
  - **Retreat:** they head home at dawn (except boss waves), or once they carry loot.
- **Hits.**
  - **Walls** lose hit points (Wall1: 25) and become their wreck at 0.
  - **Armed citizens** drop their tool and become stunned peasants.
  - **Peasants** drop coins (half lost in the water) and become beggars when broke.
  - **The monarch** drops coins the same way. With none left the crown falls off, and the monarch can't be hurt until they pick it back up. If greed touch it, the game is over.
- **Archers.** They target the toughest troll within 5.5 units and fire 2 arrows: 0.4 s to draw each, 1.3 s apart, then a 0.3 s cooldown. Arrows fly at 8.5 units/s:
  - **Arc:** the low ballistic solution, or the high one when a wall is in the way.
  - **Accuracy:** ±20% error, and 10% fall in the water.
  - **Damage:** a hit does 1.
- **Danger.** The kingdom turns unsafe at dusk. It's safe again in daylight once no attacking troll is within 20 units of the borders.

Squids, bosses, portal destruction and retaliation, knights and wall repair are covered in their own sections below.

**Not yet ported:**
- the crown's hover;
- boss red-moon and retaliation-darkness lighting;
- bosses throwing stuck boulders.

## Farming — `Farmhouse`, `Farmland`, `Farmer`, `Kingdom.ReassignFarms`

Ported in `game::farm`, with the farmer's FSM in `game::units`.
- **Seeds.** Every 10 s (`updateInterval`), a built farmhouse plants a field at `leftSeedOffset` (−1.5) and `rightSeedOffset` (+2.5), unless it's winter or it already has `maxFarmlands` (Farmhouse1: 4, Farmhouse2: 6; +2 with the farmer statue).
- **Spreading.** Each stroke a field receives can extend the chain outward by `spacing` (2.1), while the farmhouse has room. A spot blocked by a forest or any built NotFarmable region (walls, shops, castle, other farms) is skipped.
- **Growing.** A field needs 20 strokes (`farmPoints`), then drops 6 coins (`coinYield`) 0.1 s apart. The growth frame follows the `stages` curve.
- **Rot.** Unworked for 240 s (`decayInterval`), a field rots: it shows the rotten or frozen frame and its points reset. 10 s later it vanishes, unless a field further out depends on it.
- **Farmers** (walk 0.65, run 1.0):
  - Each farmer is assigned a field by distance (Hungarian), whenever fields or farmers change or the kingdom becomes safe.
  - Cycle: Idle (shuffle ±1, wait 4–5 s), then Farm (walk to the field, work 3 s, a stroke, 3 s more).
  - At night they shelter at a level-2 farmhouse, or at the campfire.
  - Outside the walls they flee from greed within 4.
  - After 4 days without farming they drop the scythe into the water and become peasants.

## Grass, thickets and wildlife — `Grass`, `World`, `PopulationController`, `Deer`, `Bunny`

Ported in `game::grass` and `game::wildlife`.
- **Grass.** The block's `Grass` seeds spread in 0.5 steps until a NotGrassable region (forest, buildings, farms) or the island edge. Every 15 s each patch grows one stage, but never more than one above its lower neighbour, so meadows are bare at the edges and tall (stage 7) in the middle. In winter they shrink, and bare edge patches disappear. Finished buildings and newly worked fields clear the grass under them. The steed eats only where the grass has grown (stage > 0).
- **Thickets.** A stage-7 patch outside the walls grows a thicket, at least 6 apart (`World.thicketSpacing`).
- **Rabbits.** Each thicket keeps ⌈10 × 0.3⌉ = 3 rabbits (one more every 25 s). They hop within ±5 of where they appeared. They have 1 HP and drop 1 coin.
- **Deer.** Each forest wider than 20 keeps ⌈width × 0.027⌉ deer (none in winter), adding one every 10 s. The FSM is Wander → Look → Eat (50 %) / WalkAway (alerted within 5 by the moving monarch or a citizen) / Wander. Archers or the moving monarch within 3 make them RunAway (back into their forest, or away for 3–4 s). They have 3 HP and drop 3 coins.
- **Archers hunting.**
  - **Targets.** Greed comes first (the most hit points). Otherwise the closest animal within 5.5 — rabbits only by day and never from a tower. The target must be reachable at `maxForce` 8.5.
  - **Chasing.** In Hunt they chase prey within 12 of the forests nearest the campfire, else roam near their border.
  - **Coins.** Afterwards they fetch coins that animals dropped within 6.
  - **Fleeing.** Outside the walls they flee from greed within 4.

## Towers — `Tower`, `GuardSlot`, `Kingdom.FetchArchersForJob`

- **Slots.** Built towers have 1 (levels 1–2), 2 (level 3) or 3 (level 4) guard slots.
- **Who goes up.** A slot that becomes free (tower built, upgraded or vacated), and every free slot at dusk, fetches the closest archer not already assigned, always keeping one archer free (`minFreeArchers`). At dusk the remaining archers split between the two walls by position (`DistributeFreeArchers`).
- **In a slot.** An archer runs to the slot and stays there for good, turning soldier: the banner-colour outfit and the `archer_soldier_*` clips. Greed can't hurt them there and they don't pick up coins. They shoot from the slot's height (rabbits excluded).
- **Leaving.** Upgrading the tower frees the slot, and an archer is fetched again for the new tower.
- **Coat of arms.** The colours come from `CoatOfArms.Randomize()` (`ColorFunctions.RandomBannerColor`, `ColorDistance`).

## Forest edges — `Forest.UpdateRegion`, `ForestItem.FadeAndRemove`

Whenever a forest's region is recomputed, decorations (items that don't `controlsForestSize`) within 1 of its edge fade out. This happens at level start and when an edge tree is chopped, and it shrinks the forest to its trees. At start we apply the cascade immediately. After a chop, removal waits `removeDelay` × U(0.5, 1.5) + 1 s.

## Castle levels, knights and shield shops — `Castle`, `Knight`, `PayableShield`, `PayableBorder`

- **Castle effects.**
  - Castle3: the closest wall (foundation, wreck or wall) within 12 on each side becomes at least level 1.
  - Castle4: a banker steps out.
  - Castle5: walls go to level 3 and towers to level 2.
  - Castle6: four shield shops appear at its placeholders. They can't be paid until the keep is built (10 s).
  - Free upgrades start as scaffolds.
- **Shield shops** (price 6).
  - A paid shield waits on its stand in the shop's banner colours (`Banner` solid/inverted rules). A peasant who touches it becomes a knight with that side and those colours, and the banner flies.
  - When the knight falls, the banner tears and the shop can be paid again.
  - A shield stolen from the stand empties the shop.
  - A hermit's knight tower adds one more shop.
- **Knights** (walk 0.6, run 1.4, purse 8, taxes above 8).
  - **States:** Assemble by the border banner on safe days, GoToWall otherwise (just inside the outermost intact wall, 1.3 + 0.25 × rank); they grab coins by day.
  - **Fighting:** they slash greed within 0.4 for 6 damage (1.2 s cooldown; the scan reaches 2.5 ahead and 0.5 behind). With the knight statue, a power slash leaps at anything in scan range.
  - **Damage:** a hit costs coins (40 % each, half lost to the water); at 1 coin or less the knight becomes a peasant.
  - **Archers:** every 10 s a knight takes up to 4 free archers within 10, always leaving one free. They follow in formation 0.3–1.5 behind it in soldier colours, hold fire while it walks out past its post, use a 1.3 s cooldown and don't hunt.
- **Border banner** (price 4). Once the castle stands and a border is more than 15 from the campfire, a banner stands 0.75 outside it. Paying it sends the knight in range (8) with the most archers to charge the nearest portal on its side.

## Portals, retaliation and victory — `Portal`, `EnemyManager`, `Director`

- **Damage.**
  - Arrows (1, or 2 when perfect) and knights (6) damage portals. Regular portals have 160 HP, cliff portals 300.
  - Archers pick the highest-HP target in range, so a portal draws their fire.
- **Defence.** An open portal with the monarch, a knight or an archer within its trigger range (8 or 10) waits 2–8 s, then sends its season's defence wave every 8 s while they stay.
- **Destruction.**
  - A regular portal crumbles: its rubble lasts 10 s and leaves a teleporter rift.
  - A cliff portal rebuilds over 3 days. Two cliff portals down at once wins the island.
- **Retaliation.** Each destroyed portal makes the kingdom unsafe and queues a retaliation wave every 2 hours, with sizes counting up from 0.
- **Borders.** Walls beyond the closest portal on a side don't count toward borders. Builders take no jobs within 12 of a portal.

## Statues and the mason shrine — `Statue`, `MasonShrine`, `Wall`

- **Charging.** Paying a statue adds a charge (max 3, rounded up). One charge lasts `chargeDurationInDays` (1 day). The time statue can only be paid when empty.
- **Blessings while charged:**
  - **Archer:** 99 % perfect shots (a third of the draw time, no scatter, double damage).
  - **Knight:** power slash.
  - **Farmer:** +2 fields per farmhouse.
  - **Worker:** walls can be repaired to 1.8× their hit points.
- **Look.** The charge marks fade as charge drains; the statue is clean while charged and overgrown again 20 s after.
- **Mason shrine.** Building the stone shrine unlocks stone. It isn't on Land One; Land Two onward has one.
- **Wall repair.** Damaged walls become builders' jobs when the kingdom is safe (or when hit while safe): +`repairRate` per stroke below full, `repairRateAboveMax` above, with priority −20.

## Mounts — `Steed`, `SteedSpawn`, `Player.Ride`

- **Spawns.** Each spawn place puts out `amount` distinct steeds from its list:
  - Meadow: 2 horses.
  - Bear cave: the bear.
  - Blossom tree: the unicorn.
  - King's grave: the warhorse.
  - Mansion: the spooky horse.
  - Waterfall: the stag.
  - Santa's house: the reindeer.
- **Free steeds.** They wander within `wanderRange` of home at half walking speed, eat, then rest 10–15 s. You can pay for one (3) unless it's walking; the old steed is left free where the monarch was.
- **Abilities:**
  - **Bear:** swipes for 2 while galloping, at 0.2 stamina per hit.
  - **Unicorn:** drops 3 coins for the monarch when it finishes eating, after its 30 s cooldown.
  - **Warhorse:** starting a gallop inspires citizens within 5 for 12 s (hits don't cost them coins or tools); its mane takes the banner's secondary colour.
  - **Stag and reindeer:** attract deer, which follow instead of fleeing.
  - **Spooky horse:** needs no grass, but you can't switch away from it.

## Hermits — `Cabin`, `Hermit`, `Baker`, `Ballista`, `TowerKnight`

- **Getting one.**
  - Paying a cabin (4) sends its hermit out. The hermit walks back inside the walls and stands where no other price tag is, moving 4 outward when crowded.
  - Paying a hermit (1) puts it behind the monarch's saddle (the steed's `Passenger` spot).
  - The hermit hops off when the monarch is hit or switches steeds, or when the monarch leaves the kingdom (±5) at night.
- **Using one.** A level-4 tower paid with a hermit aboard becomes that hermit's tower (`passengerUpgrades`), and the hermit hops off to build it:
  - **Baker:** sells bread (1 each, rack of 7). Every 20 s it calls the closest beggar not yet called, up to 3 and no more than the loaves. The beggar leaves its camp, walks over and eats a loaf.
  - **Ballista:** one builder stands on it and winds it in 7 strokes. Loaded, it aims at the toughest greed 1.2–10 in front (low arc at 20, clamped to ±30°, turning 40°/s) and fires a bolt that pierces up to 8 enemies for 12 each.
  - **Knight:** a shield shop at its foot.

## The merchant — `MerchantSpawner`, `Merchant`, `Mule`

Once the castle stands, the merchant house sends a merchant with a mule carrying 8 coins (× `incomeMultiplier`):
- He waits by the campfire for the morning, then drops the coins (for the monarch) when the monarch stands within 2.
- Afterwards he waits somewhere free of price tags. Paying him 1 sends him home to restock (2 hours) and back.
- A troll hitting the mule knocks a coin off.

## Chests and the banker — `Chest`, `Banker`

- **Chests.** One spills its 12 coins (× `incomeMultiplier`) for the monarch when the monarch stops at it (not galloping), then fades after 10 s.
- **The banker** (from Castle4):
  - **Deposits:** he picks up coins the monarch drops (purse of 10) and carries them into the castle.
  - **Interest:** each dawn adds `min(8, ⌈7 % × stash⌉)`.
  - **Payout:** standing still by him pays out ⌈⅓ × stash⌉, keeping the monarch at or under 35. Payouts of 2 or fewer aren't made, there's at most one per day, and none after he took 2+ coins that day.
  - **Hiding:** he hides in the castle at night or with greed within 5.

## Workshop and catapults — `PayableWorkshop`, `Catapult`, `Boulder`

- **Getting one.** The workshop (from Castle4) has a counter on each side (6). Paying a counter orders a catapult, which builders put up (10 strokes).
- **Builders' work.** Two builders push it outward at 0.15 per pusher until it's within 2.5 of the wall, then crank it: 10 strokes of 6.8° up to 68°.
- **Firing.** Loaded, it fires at greed 2–7.5 in front: a boulder at 7–9 along (1, 1.5) does 15 to each greed it passes (after 0.3 s). It bounces once, then sticks (50 %) or bursts.
- **Defence.** Trolls treat catapults as priority targets. A catapult has 4 HP.

## Teleporters — `PayableTeleporter`, `TeleporterExit`, `TeleporterRift`

- **Building one.** A portal's rift upgrades into a teleporter (5, then 40 builder strokes).
- **First trip.** The monarch steers an exit point (15/s, 15 s timeout) and leaves it with the pay button. The monarch arrives there, and a rift is left if no price tag is within 1. That rift vanishes after 30 s unless it's built up, which links the two teleporters.
- **Linked trips.** Paying a linked teleporter (1, 70 s cooldown) moves the monarch to the other end.

## The dog — `DogSpawn`, `Dog`

- **Rescue.** A coin dropped onto the dog trapped under a tree frees it, in one of three coat colours.
- **Behaviour.**
  - It follows the monarch when the monarch gallops or is more than 10 away; otherwise it wanders ±5 and rests.
  - When a wave sets out it barks toward the nearest portal, from the monarch's side.
  - It cowers by the monarch while greed are within 10.

## Squids and bosses — `Squid`, `Boss`

- **Squids** (22 HP, 1.3/s):
  - They fly at 2.2 over the walls toward the campfire and swoop on citizens within 5.
  - A grabbed armed citizen drops coins and tool first. Knights can't be grabbed.
  - A squid carries up to 2, then heads for a portal, and the victims are lost. Shot down, it releases them as peasants.
- **Bosses** (90 HP; 5000 for killer bosses; 1.2/s):
  - They march to the campfire, then after the crowned monarch.
  - They bash everything 0–1.6 in front for 22.
  - They spit 4 trolls when hurt or when the monarch is within 8 ahead (every 10 s).
  - They die instead of retreating. Like their whole wave, they don't retreat at dawn.

## The wharf, the boat and the next island — `Wharf`, `BoatPart`, `Boat`, `BoatPushPosition`, `Beach`, `UnlockManager`, `Signpost`

- **The wharf.** A `Wreck` (in the `Boat` block, or by the spawn on later islands) upgrades into the `Wharf` for 10.
  - It faces the beach and hides its 59 boat parts. Land One stacks 30 of them for free (`freeBoatParts`).
  - Each payment of 2 drops the next part onto the pile. Up to 3 builders work at ±2 around it; every 4 strokes (`workPerPart`) fit the oldest stacked part, which flies to its place on the hull and appears 0.5 s later.
  - A troll can carry off a stacked part; it goes back to the parts still to be paid for.
  - With nothing left to pay for and nothing stacked, one more payment launches the boat. It drops from the skeleton base onto the water (`waterHeight`, 1.5 s, OutBack).
- **Pushing.** The boat has 3 push positions, one builder each, standing 5/32 behind them with the push animation.
  - It moves toward the beach at 0.12 per pusher (×0.5 in a forest). Within 0.125 of the beach it waits.
  - Builders still keep 12 back from the closest portal (`Kingdom.workPortalDistance`), so a portal between the wharf and the beach has to fall first.
- **Boarding.** While it waits, up to 3 builders walk to the stern (`DeckLeft`) and step aboard.
  - Knights board too: paying the border banner on the boat's side, with no portal standing there, sends a knight (and its archers) to escort the boat 2–4 ahead while it's pushed. Once it waits, they board, 3 knights at most.
  - Archers man the boat's 4 guard slots like a tower's.
- **Sailing away.** The beach (10) is payable once the boat waits.
  - Paying it opens the sail and wins the island. Archers on the boat's slots and the dog come along, and the monarch rides on deck (1.40625 from the middle).
  - The boat accelerates by 0.2/s² to 0.7.
  - While a boat exists, regular (non-boss) waves are ×2 (`waveBoatMultiplier`).
- **The next island** (`QueueLand` / `Restore`). It's the next Land (after Five, back to One). The new island starts with:
  - the steed being ridden, the coins and the coat of arms;
  - the hermit riding along and the dog;
  - one archer, builder and knight for each that was aboard, all within 2 of the monarch;
  - the reign, plus one.
- **Unlocks** (`UnlockStorage`, kept across runs).
  - Each of the 13 unlockables (four statues, meadow, grave, cave, waterfall, sakura, dog, three hermits) belongs to one island (`unlockableOnLand`).
  - Blocks marked `needsUnlock` only appear once theirs is unlocked.
  - The signpost (2) offers its map while something on its island is still locked. Paying takes the map and picks one at random.
  - It's unlocked when the run ends, won or lost. For the next three islands its block is forced into the layout (+5 to `RequiresPlace`).
  - Winning an island reveals the next; winning Land Five finishes the game and reveals Land Six.

## The purse — `CoinBag`, `BagCoin`, `Player.CoinFellFromBag`

- **Coins in the bag are bodies.** Each coin is a rigid body (unit mass, gravity ×0.6, linear drag 0.7, angular drag 0.05; `BagCoins` material: friction 0.1, bounce 0.5). It drops in from (±0.2, 1) at −30..30°, one every 0.15 s.
- **Two piles.** The first 8 coins and every even one after go in the back pile (shaded ×0.9), the rest in the front pile. The two piles pass through each other and only meet the bag.
- **Spills.** Past 300 coins, new ones don't stack at all. A coin that goes over the rim and reaches the `Ground` trigger has fallen out: the monarch drops a real coin, and half the time it lands in the water and is lost (`overflowCoinsWaterProbability`).
- **Spending.** Spending removes the newest coins first.
- **Blinks.** A coin blinks white 0.5 s after it first lands. While the purse holds 8 or fewer, they blink left to right every 2 s.
- The bag holds about 55 coins before it starts to spill (to be checked against the original).

## The island map — `Map`, `MapLand`, `MapUnlock`, `MapCloud`

- **After a win** (`Mode.Continue`): any island reached except the current one. It starts on the island the boat was headed for, and Land Six can be picked once reached.
- **After a loss** (`Mode.Restart`): any island reached except Land Six, starting on the current one.
- **Display.** Islands not reached yet are under clouds, and each unlocked unlockable stands as a figurine on its island. The boat (or wreck, after a loss) waits by the island just left, and the pointer marks the pick.

## Saving — `PersistentManager`, `UnlockManager`

- **When.**
  - Autosave 2 s after the kingdom becomes safe, from the second day on (`HandleOnBecameSafe`).
  - On landing on a new island (`Restore`), and on quitting (`OnButtonQuit`). The pause menu's Save is F5 on desktop.
  - A lost run's save is deleted (`EndGame` → `DeleteSavedGame`).
- **Unlocks** are stored separately (`unlocks`) when a run ends, won or lost.
- **What.** `game::save` writes the whole `GameState` (postcard) behind a magic, a format version and a fingerprint of `world.bin`. A save made with different game data is refused instead of misread. Saves run 60–100 KB.
- **Where.** The backend decides (`platform::Storage`): desktop files under `~/.local/share/kingdom-3ds/` (or `$KINGDOM_SAVES`), later the 3DS SD card.

## Rendering facts

- **Scale.** 32 px per unit (`ScreenManager.PixelsPerUnit`). The original shows 12–16 units across and 6–7.5 tall. The 3DS top screen at 1:1 is 12.5 × 7.5 units, so it fits natively.
- **Sprite atlases.** Sprites are tightly packed as **polygons** in Unity's atlases. `tools/extract` cuts each one out by its mesh and repacks everything into ≤1024² pages.
- **Animation.** Clips swap sprites at 10 fps. The steed's Animator drives the root sprite plus the `King`/`Queen` (rider), `King/Crown` and `Puff` children.
  - In clips that don't key the rider (idle, eat), the rider keeps the prefab default `player_king_eat_0`.
  - The `player_king_*` base clips use hashed paths (`Rider`, `Crown`, `Torch`; CRC32) that don't exist under the steed prefabs, so they have no effect.
- **Recolor.** Sprite alpha is a recolour key; see `docs/deviations.md`. Rider: A = coat-of-arms primary, B = skin. Steed: A = `maneColor`, B = `coatColor`. The rider is the steed's `King` or `Queen` child (`Look::queen`): its own clip track, default sprite and crown (`crowns_1` / `crowns_2`).

## Remaining systems (mapped by class name; details pending)

| Area | Original classes | Rust module |
|---|---|---|
| Monarch & movement | `Player`, `Mover`, `Crown`, `MainCamera` | `game::player` |
| Mounts | `Steed` (Payable), `SteedSpawn`, `TriggerRearing` | `game::mount` |
| Dog | `Dog`, `DogSpawn` | `game::dog` |
| Population | `Character` (shared base behaviour), `Beggar`, `BeggarCamp`, `Peasant`, `Worker` (builder), `Archer`, `Farmer`, `Knight`, `TowerKnight`, `PopulationController`, `JobAssigner` | `game::units::*` |
| Tools / roles | `DroppableTool`, `DroppableShield`, `PayableWorkshop`, `PayableShop` | `game::units::roles` |
| Building | `Workable`, `WorkableBuilding`, `WorkableTree`, `Wall`, `Tower`, `Scaffolding`, `ScaffoldModule`, `PayableUpgrade`, `PayableTree`, `Castle`, `Farmhouse`, `Farmland`, `Ballista`, `Catapult`, `GuardSlot` | `game::build` |
| Kingdom state | `Kingdom` (borders, safety, unit lists, statue flags), `PayableBorder`, `Banner` | `game::kingdom` |
| Greed & waves | `Enemy`, `EnemyData`, `EnemyManager`, `EnemyBlueprint`, `Wave`, `WaveType`, `NamedWave`, `Horde`, `Portal`, `Troll`, `Squid`, `Boss`, `Damageable`, `DamageSource`, `Engagement` | `game::greed` |
| Losing the crown / game over | `Crown`, `Game` (`EndGame`, `Result`), `TriggerCrownInWater` | `game::session` |
| Islands & boat | `Game`, `LevelConfig`, `Level`, `LevelLayout`, `LevelBlock*`, `Land`, `NextLandData`, `Map`, `MapLand` (`Boat`, `Wharf` etc.: above) | `game::level`, `game::boat`, `game::progress` |
| Merchants | `Merchant`, `MerchantSpawner`, `Mule` | `game::merchant` |
| Shrines / statues | `Statue` (archer/worker/knight/farmer/time charges in `Kingdom`), `MasonShrine`, `PayableTeleporter`, `TeleporterExit`, `TeleporterRift` | `game::shrines` |
| Hermits | `Hermit` (Payable, rides as `Player.passenger`), `Cabin` | `game::hermit` |
| Banker / chests | `Banker`, `Chest` | `game::economy` |
| Unlocks | `UnlockManager`, `Unlockable`, `UnlockStorage` (above) | `game::progress` |
| World / environment | `World` (seasons, wind, forests, grass), `Forest`, `Grass`, `Foliage`, `Ground`, `Tile`, `River`, `Water`, `Waterfall`, `Precipitation`, `Snow`, wildlife (`Deer`, `Bunny`, `Bird`) | `game::world` |
| Lighting | `Blender`, `BlenderState` (weather × season × part of day), `BlenderStateFader`, `LightFaker`, `ReflectedLight`, `Glow`, `CelestialBody`, `Stars` | renderer-side; `game::time` exposes the blend inputs |
| Parallax / rendering | `Parallax2D`, `ContentLayers`, `Layers`, `SpriteFX` | `platform::Renderer` + backends |
| Audio | `AudioManager`, `AudioEmitter`, `AudioPool`, `TimeBasedAudio`, `Day.TrackPlay` | `platform::Audio` |
| Saves | `PersistentManager`, `IPersistent`, `*Data` classes, `Filer`, `PrefsStore` (above) | `game::save` + `platform::Storage` |
| Seasonal events | `Christmas*`, `Snowman`, `SeasonalSprite*` | later / optional |
| Tutorial | `TutorialManager`, `TutorialTrail`, `TutorialWall`, `Hint*` | later |

Ignored: Steam/Xbox/PS4/Switch/Android/Apple platform managers, `GooglePlayGames`, `nn.*`, `Rewired*`, `ParticlePlayground` (we'll replace its effects with simple particles), `ConsoleProDebug`, `DebugTools` (useful as a reference for our own debug overlay).

---

## Where the scene values come from

All **(scene)** values above are confirmed in the AssetRipper YAML export:
- `Director` and `Kingdom`: `reference/ripped/ExportedProject/Assets/main.unity` (around line 20450).
- `Player`: `Assets/Resources/prefabs/characters/Player.prefab`.

To check any other tuning value, grep the field name across `main.unity`, `Assets/GameObject/` and `Assets/Resources/`.

Useful data for the extract tool, in `Assets/Resources/`:
- `days/`: the `Day` assets.
- `waves/`: the `Wave` assets.
- `levels/`: the `LevelConfig` assets.
- `blender/`: the `BlenderState` lighting presets, named `<Weather> <Season> <PartOfDay>`. These are the natural source for the Old 3DS per-phase tint table.
- `prefabs/`: everything spawned at runtime.

Sprite sheets are in `Assets/Texture2D/` (86 sheets), with per-sprite rects/pivots in `Assets/Sprite/` (2200). Audio is in `Assets/AudioClip/`.
