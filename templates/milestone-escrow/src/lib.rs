// MilestoneEscrow, an escrow that ships with the Verdict court built in.
//
// A client funds an agreement with milestone amounts. The freelancer marks
// each milestone delivered, the client approves, and approval releases
// exactly that milestone's slice to the payee. Unreleased money is
// refundable to the payer after the deadline. The escrow holds its own
// vault, and its settlement methods accept calls from exactly one
// component, the arbitrator (Verdict court) named at creation. The court
// never touches the vault; the escrow's own code moves it.
//
// Access control, three layers:
//   1. Access rule: rule!(caller_component(arbitrator)) on freeze_for_dispute
//      and execute_ruling.
//   2. In-method assert: CallerContext::assert_badge_in_scope with the
//      caller-component badge of the arbitrator. There is no direct
//      "caller address" getter in tari_template_lib 0.41, so the badge
//      assert is the in-method form of the same check.
//   3. OwnerRule::None on the production constructor, so no owner path can
//      bypass the rule.
//
// The production constructor `new` is the only constructor. There are no
// verification constructors: the template's `#[template]` macro generates
// a dispatcher arm for every public function, and it does not respect
// `#[cfg]`, so a test-only constructor gated by a cargo feature would
// still be dispatched in a production build and break it. The escrow's
// own guards (frozen, dispute id, amounts, ruling shape) are defense in
// depth behind the court, verified by audit, and the end-to-end tests
// prove legitimate rulings pass them.
//
// Deadline semantics, by method. The deadline is an epoch, set at creation,
// and must be after the creation epoch. Only refund is deadline-limited in
// the sense that it requires the deadline to have passed; every other
// party method works before the deadline and stops working after it, so
// the deadline is the point where the deal stops moving forward and the
// payer's refund path opens:
//   - fund: before the deadline only. After the deadline, funding stops.
//   - mark_delivered: before the deadline only.
//   - approve: before the deadline only. Approval after the deadline is
//     refused, so a late approval cannot race the payer's refund.
//   - cancel: before the deadline only. After the deadline, refund is the
//     closing path, not cancel.
//   - refund: after the deadline only (current epoch >= deadline). The
//     only method that requires the deadline to have passed.
//   - freeze_for_dispute: any time, while the agreement is open. A dispute
//     can open before or after the deadline.
//   - execute_ruling: any time, while frozen for this dispute. A ruling
//     supersedes the deadline, so the court can settle a dispute that
//     opened after the deadline.
//   - A frozen dispute blocks every state change: fund, mark_delivered,
//     approve, cancel, and refund all refuse while frozen, including a
//     refund that would drain the vault while the court is ruling.
//
// Terminal states. A fully released agreement closes: once every milestone
// has been released, the agreement is complete and no further state change
// is possible. A ruling that has executed, a cancel, and a refund are also
// terminal. Cancel and refund handle the empty-vault case: cancel on an
// empty vault closes the agreement and returns an empty bucket, and refund
// on an empty vault is refused, since there is nothing to return.
//
// Arithmetic is checked everywhere: milestone totals, funded totals, and
// released totals use checked addition, and every withdrawal is checked
// against the vault balance before it moves. No unchecked addition, and no
// unchecked cast, appears in this template.
//
// execute_ruling is the only settlement path. It withdraws from the vault
// directly and never reuses approve or any other payer-only method, so no
// settlement step depends on the transaction signer.

use tari_template_lib::prelude::*;

#[template]
mod milestone_escrow {
    use super::*;

    /// Ruling outcome values, matching the court's vote values:
    /// pay 0, refund 1, split 2, no ruling 4.
    const OUTCOME_PAY: u64 = 0;
    const OUTCOME_REFUND: u64 = 1;
    const OUTCOME_SPLIT: u64 = 2;
    const OUTCOME_NO_RULING: u64 = 4;

    /// The largest juror fee total the court can take from the
    /// principal: one fee per juror in the first-round majority, and
    /// the first-round panel has three jurors, so at most three fees.
    /// The court pays appeal jurors from the appeal fee, not from the
    /// principal, so the principal never funds more than this. This
    /// must stay equal to the court template's PANEL_SIZE * JUROR_FEE
    /// (3 * 5,000). The escrow uses it to bound the payer's gross
    /// amount on a pay ruling, which is exactly the fee, and to bound
    /// the split, where the payer's gross is the fee plus the payer's
    /// half of the post-fee principal.
    const MAX_PRINCIPAL_FEE: u64 = 15_000;

    pub struct MilestoneEscrow {
        /// The court component named at creation. Immutable, never changed.
        arbitrator: ComponentAddress,
        /// Public key of the payer who created (and funds) the escrow.
        payer: RistrettoPublicKeyBytes,
        /// Public key of the payee.
        payee: RistrettoPublicKeyBytes,
        /// Escrow vault holding the funded TARI until a release, a ruling,
        /// a refund, or a cancel moves it.
        vault: Vault,
        /// The milestones of this agreement, in order.
        milestones: Vec<Milestone>,
        /// Sum of all milestone amounts, in microTari. Checked at creation.
        total: u64,
        /// Epoch after which the payer may refund the unreleased remainder.
        deadline: u64,
        /// Total funded so far, in microTari. Never exceeds total.
        funded: u64,
        /// Total released to the payee so far, in microTari.
        released: u64,
        /// True while a dispute is open on this escrow.
        frozen: bool,
        /// Dispute id the vault is frozen for (0 when never frozen).
        frozen_dispute_id: u64,
        /// True once a ruling has executed. A ruling executes at most once.
        ruling_executed: bool,
        /// True once the payer has cancelled the agreement.
        cancelled: bool,
        /// True once the payer has refunded the unreleased remainder.
        refunded: bool,
        /// When false the in-method caller assert is skipped
        /// (used by the owned twin so the owner bypass is observable).
        assert_caller: bool,
    }

    pub struct Milestone {
        /// Amount held for this milestone, in microTari.
        pub amount: u64,
        /// True once the payee has marked this milestone delivered.
        pub delivered: bool,
        /// True once the payer has approved this milestone and its slice
        /// has been released.
        pub released: bool,
    }

    impl MilestoneEscrow {
        /// Production form: ownerless, caller rule, in-method assert on.
        /// This is the only constructor. It takes no configuration: the
        /// owner rule, the assert, and the access rules are fixed here,
        /// so no caller can create a weakened escrow.
        pub fn new(
            arbitrator: ComponentAddress,
            payee: RistrettoPublicKeyBytes,
            milestone_amounts: Vec<u64>,
            deadline: u64,
        ) -> Component<Self> {
            let state = Self::build_state(arbitrator, payee, milestone_amounts, deadline);
            let rules = AccessRules::allow_all()
                .add_method_rule("freeze_for_dispute", rule!(caller_component(arbitrator)))
                .add_method_rule("execute_ruling", rule!(caller_component(arbitrator)));
            Component::new(state)
                .with_access_rules(rules)
                .with_owner_rule(OwnerRule::None)
                .create()
        }

        /// Validate the creation inputs and build the escrow state.
        /// The in-method caller assert is on.
        fn build_state(
            arbitrator: ComponentAddress,
            payee: RistrettoPublicKeyBytes,
            milestone_amounts: Vec<u64>,
            deadline: u64,
        ) -> Self {
            let payer = CallerContext::transaction_signer_public_key();
            assert!(
                payee != payer,
                "payee must differ from the payer (the creator)"
            );
            assert!(
                !milestone_amounts.is_empty(),
                "at least one milestone is required"
            );
            assert!(
                deadline > Consensus::current_epoch(),
                "deadline must be after the current epoch"
            );
            let mut total: u64 = 0;
            let mut milestones: Vec<Milestone> = Vec::new();
            for amount in milestone_amounts {
                assert!(amount > 0, "milestone amount must be positive");
                total = total
                    .checked_add(amount)
                    .expect("milestone amounts overflow the total");
                milestones.push(Milestone {
                    amount,
                    delivered: false,
                    released: false,
                });
            }
            Self {
                arbitrator,
                payer,
                payee,
                vault: Vault::new_empty(TARI_TOKEN),
                milestones,
                total,
                deadline,
                funded: 0,
                released: 0,
                frozen: false,
                frozen_dispute_id: 0,
                ruling_executed: false,
                cancelled: false,
                refunded: false,
                assert_caller: true,
            }
        }

        /// Fund the escrow. Only the payer may fund, before the deadline,
        /// while the agreement is open. Funding may be partial, and may
        /// arrive in several calls, but the funded total never exceeds the
        /// milestone total.
        pub fn fund(&mut self, bucket: Bucket) {
            self.assert_open();
            self.assert_before_deadline("funding is closed after the deadline");
            assert!(
                CallerContext::transaction_signer_public_key() == self.payer,
                "only the payer can fund the escrow"
            );
            assert!(
                bucket.resource_address() == TARI_TOKEN,
                "escrow is funded in TARI only"
            );
            let amount = bucket
                .amount()
                .to_u64_checked()
                .expect("bucket amount out of range");
            assert!(amount > 0, "funding bucket must not be empty");
            let funded = self
                .funded
                .checked_add(amount)
                .expect("funded total overflow");
            assert!(funded <= self.total, "funding exceeds the milestone total");
            self.funded = funded;
            self.vault.deposit(bucket);
        }

        /// Mark a milestone delivered. Only the payee may mark it, before
        /// the deadline, while the agreement is open. A milestone can be
        /// marked delivered once.
        pub fn mark_delivered(&mut self, index: u64) {
            self.assert_open();
            self.assert_before_deadline("delivery is closed after the deadline");
            assert!(
                CallerContext::transaction_signer_public_key() == self.payee,
                "only the payee can mark a milestone delivered"
            );
            let milestone = self.milestone_mut(index);
            assert!(!milestone.delivered, "milestone is already delivered");
            assert!(!milestone.released, "milestone is already released");
            milestone.delivered = true;
        }

        /// Approve a delivered milestone. Only the payer may approve,
        /// before the deadline, while the agreement is open. Approval
        /// releases exactly that milestone's slice to the payee's account,
        /// and the slice must be funded before it can move. The escrow
        /// deposits the slice itself, so the caller cannot misroute it.
        pub fn approve(&mut self, index: u64, payee_account: ComponentAddress) {
            self.assert_open();
            self.assert_before_deadline("approval is closed after the deadline");
            assert!(
                CallerContext::transaction_signer_public_key() == self.payer,
                "only the payer can approve a milestone"
            );
            let amount = {
                let milestone = self.milestone_mut(index);
                assert!(milestone.delivered, "milestone is not delivered");
                assert!(!milestone.released, "milestone is already released");
                milestone.released = true;
                milestone.amount
            };
            let balance = self
                .vault
                .balance()
                .to_u64_checked()
                .expect("vault balance out of range");
            assert!(amount <= balance, "milestone slice is not funded yet");
            self.released = self
                .released
                .checked_add(amount)
                .expect("released total overflow");
            let bucket = self.vault.withdraw(Amount::from_u64(amount));
            ComponentManager::get(payee_account).invoke("deposit", args![bucket]);
        }

        /// Cancel the agreement. Only the payer may cancel, before the
        /// deadline, while the agreement is open. Cancelling returns the
        /// unreleased remainder to the payer as a bucket and closes the
        /// agreement. Cancelling an empty vault closes the agreement and
        /// returns an empty bucket.
        pub fn cancel(&mut self) -> Bucket {
            self.assert_open();
            self.assert_before_deadline("cancel is closed after the deadline");
            assert!(
                CallerContext::transaction_signer_public_key() == self.payer,
                "only the payer can cancel the escrow"
            );
            self.cancelled = true;
            let balance = self
                .vault
                .balance()
                .to_u64_checked()
                .expect("vault balance out of range");
            self.vault.withdraw(Amount::from_u64(balance))
        }

        /// Refund the unreleased remainder to the payer. Only the payer
        /// may refund, and only after the deadline (current epoch >=
        /// deadline), while the agreement is open and not frozen. A
        /// frozen dispute blocks refund, so a refund cannot drain the
        /// vault while the court is ruling. Refund on an empty vault is
        /// refused, since there is nothing to return.
        pub fn refund(&mut self) -> Bucket {
            self.assert_open();
            assert!(
                Consensus::current_epoch() >= self.deadline,
                "refund is only available after the deadline"
            );
            assert!(
                CallerContext::transaction_signer_public_key() == self.payer,
                "only the payer can refund the escrow"
            );
            let balance = self
                .vault
                .balance()
                .to_u64_checked()
                .expect("vault balance out of range");
            assert!(balance > 0, "vault is empty, nothing to refund");
            self.refunded = true;
            self.vault.withdraw(Amount::from_u64(balance))
        }

        /// Freeze the vault for a dispute. Only the arbitrator component.
        /// Works any time while the agreement is open, so a dispute can
        /// open before or after the deadline.
        pub fn freeze_for_dispute(&mut self, dispute_id: u64) {
            self.assert_arbitrator();
            assert!(!self.frozen, "escrow is already frozen for a dispute");
            assert!(
                !self.ruling_executed,
                "a ruling has already executed on this escrow"
            );
            assert!(!self.cancelled, "escrow is cancelled");
            assert!(!self.refunded, "escrow is refunded");
            assert!(!self.is_complete(), "escrow is complete");
            let balance = self
                .vault
                .balance()
                .to_u64_checked()
                .expect("vault balance out of range");
            assert!(balance > 0, "escrow vault is empty, nothing to dispute");
            self.frozen = true;
            self.frozen_dispute_id = dispute_id;
        }

        /// Execute a ruling: move the escrow's own vault per the ruling.
        /// Only the arbitrator component, only while frozen for this
        /// dispute, exactly once. Takes the outcome and the amounts pair
        /// (payee amount, payer amount), where the payer amount is the
        /// gross amount routed to the payer side, before the court takes
        /// juror fees out of it, and the payee amount is net. Returns
        /// both buckets in one atomic call, (payee bucket, payer bucket),
        /// so no funds stay in the vault and no second call is needed.
        /// The ruling is validated with checked arithmetic: the outcome
        /// must be pay, refund, split, or no ruling; the two amounts
        /// must sum to the vault balance exactly; a refund and a
        /// no-ruling ruling pay the payee nothing. A ruling that fails
        /// any of these is refused and leaves the escrow untouched.
        /// Conservation is exact: payee bucket + payer bucket == vault
        /// balance. This is the only settlement path: it withdraws from
        /// the vault directly and never reuses approve or any other
        /// payer-only method.
        pub fn execute_ruling(
            &mut self,
            dispute_id: u64,
            outcome: u64,
            amounts: (u64, u64),
        ) -> (Bucket, Bucket) {
            self.assert_arbitrator();
            assert!(!self.ruling_executed, "ruling already executed");
            assert!(self.frozen, "escrow is not frozen for a dispute");
            assert_eq!(
                dispute_id, self.frozen_dispute_id,
                "dispute id does not match the frozen dispute"
            );
            assert!(
                outcome == OUTCOME_PAY
                    || outcome == OUTCOME_REFUND
                    || outcome == OUTCOME_SPLIT
                    || outcome == OUTCOME_NO_RULING,
                "invalid ruling outcome"
            );
            let (payee_amount, payer_amount) = amounts;
            if outcome == OUTCOME_REFUND {
                assert!(payee_amount == 0, "refund pays the payee nothing");
            }
            if outcome == OUTCOME_NO_RULING {
                assert!(payee_amount == 0, "no ruling pays the payee nothing");
            }
            let balance = self
                .vault
                .balance()
                .to_u64_checked()
                .expect("vault balance out of range");
            let total = payee_amount
                .checked_add(payer_amount)
                .expect("ruling amounts overflow");
            assert!(
                total == balance,
                "ruling amounts must equal the vault balance"
            );
            // The pay and split cells are validated by shape, after
            // conservation. The court computes the exact fee, but the
            // escrow knows the fee's bounds, so a ruling that
            // contradicts them is refused even though it conserves:
            //   - Pay: the payer's gross amount is exactly the juror
            //     fee, which is at most MAX_PRINCIPAL_FEE. A pay that
            //     hands the whole principal to the payer is a refund
            //     wearing a pay outcome, and it is refused.
            //   - Split: the payer's gross is the fee plus the payer's
            //     half of the post-fee principal, and the payee gets
            //     the other half, rounded up. So the payee is never
            //     more than one micro-Tari above the payer, and the
            //     payer is never more than the maximum fee above the
            //     payee. A split that hands everything to one side is
            //     refused. The saturating subtraction keeps the
            //     comparison from overflowing on a u64 boundary.
            if outcome == OUTCOME_PAY {
                assert!(
                    payer_amount <= MAX_PRINCIPAL_FEE,
                    "pay fee exceeds the maximum juror fee total"
                );
            }
            if outcome == OUTCOME_SPLIT {
                assert!(
                    payee_amount.saturating_sub(payer_amount) <= 1,
                    "split payee exceeds the payer by more than one micro-Tari"
                );
                assert!(
                    payer_amount.saturating_sub(payee_amount) <= MAX_PRINCIPAL_FEE,
                    "split payer exceeds the payee by more than the maximum juror fee total"
                );
            }
            self.ruling_executed = true;
            self.frozen = false;
            self.released = self
                .released
                .checked_add(total)
                .expect("released total overflow");
            (
                self.vault.withdraw(Amount::from_u64(payee_amount)),
                self.vault.withdraw(Amount::from_u64(payer_amount)),
            )
        }

        // ----- read-only getters -----

        pub fn arbitrator(&self) -> ComponentAddress {
            self.arbitrator
        }

        pub fn payer(&self) -> RistrettoPublicKeyBytes {
            self.payer
        }

        pub fn payee(&self) -> RistrettoPublicKeyBytes {
            self.payee
        }

        pub fn vault_balance(&self) -> u64 {
            self.vault.balance().to_u64_checked().unwrap_or(0)
        }

        pub fn is_frozen(&self) -> bool {
            self.frozen
        }

        pub fn ruling_executed(&self) -> bool {
            self.ruling_executed
        }

        pub fn is_cancelled(&self) -> bool {
            self.cancelled
        }

        pub fn is_refunded(&self) -> bool {
            self.refunded
        }

        pub fn is_complete(&self) -> bool {
            self.released == self.total || self.ruling_executed
        }

        /// True once the escrow can no longer change state: completed,
        /// ruled, cancelled, or refunded.
        pub fn is_terminal(&self) -> bool {
            self.is_complete() || self.cancelled || self.refunded
        }

        pub fn deadline(&self) -> u64 {
            self.deadline
        }

        pub fn total_amount(&self) -> u64 {
            self.total
        }

        pub fn funded_total(&self) -> u64 {
            self.funded
        }

        pub fn released_total(&self) -> u64 {
            self.released
        }

        pub fn milestone_count(&self) -> u64 {
            u64::try_from(self.milestones.len()).expect("milestone count out of range")
        }

        pub fn milestone_amount(&self, index: u64) -> u64 {
            self.milestone(index).amount
        }

        pub fn milestone_delivered(&self, index: u64) -> bool {
            self.milestone(index).delivered
        }

        pub fn milestone_released(&self, index: u64) -> bool {
            self.milestone(index).released
        }

        // ----- helpers -----

        fn milestone(&self, index: u64) -> &Milestone {
            self.milestones
                .get(usize::try_from(index).expect("milestone index out of range"))
                .expect("milestone index out of range")
        }

        fn milestone_mut(&mut self, index: u64) -> &mut Milestone {
            self.milestones
                .get_mut(usize::try_from(index).expect("milestone index out of range"))
                .expect("milestone index out of range")
        }

        /// The agreement is open for party state changes: not frozen, not
        /// ruled on, not cancelled, not refunded, and not complete.
        fn assert_open(&self) {
            assert!(!self.frozen, "escrow is frozen for a dispute");
            assert!(
                !self.ruling_executed,
                "a ruling has already executed on this escrow"
            );
            assert!(!self.cancelled, "escrow is cancelled");
            assert!(!self.refunded, "escrow is refunded");
            assert!(!self.is_complete(), "escrow is complete");
        }

        fn assert_before_deadline(&self, message: &str) {
            assert!(Consensus::current_epoch() < self.deadline, "{message}");
        }

        fn assert_arbitrator(&self) {
            if self.assert_caller {
                CallerContext::assert_badge_in_scope(&NonFungibleAddress::caller_component_badge(
                    self.arbitrator,
                ));
            }
        }
    }
}
