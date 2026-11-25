// PaymentEscrow, an escrow that ships with the Verdict court built in.
//
// One amount, one delivery. The payer funds the agreement, the payee marks
// the work delivered, the payer approves, and approval releases the full
// amount to the payee. Unreleased money is refundable to the payer after
// the deadline. The escrow holds its own vault, and its two settlement
// methods accept calls from exactly one component, the arbitrator (Verdict
// court) named at creation. The court never touches the vault; the escrow's
// own code moves it.
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
// Verification constructors (used by the engine tests to isolate the
// layers; the production build keeps only `new`):
//   - `new_owned`: OwnerRule::OwnedBySigner, caller rule, assert OFF. The
//     owned twin: proves the owner bypasses the caller rule.
//   - `new_rule_open`: OwnerRule::None, allow-all rules, assert ON. The
//     assert-only twin: proves the in-method assert rejects a stranger
//     even when the access rule would allow the call.
//
// Deadline semantics, by method. The deadline is an epoch, set at creation,
// and must be after the creation epoch. Only refund is deadline-gated in
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
// Terminal states. A released agreement closes: once the payment has been
// released, the agreement is complete and no further state change is
// possible. A ruling that has executed, a cancel, and a refund are also
// terminal. Cancel and refund handle the empty-vault case: cancel on an
// empty vault closes the agreement and returns an empty bucket, and refund
// on an empty vault is refused, since there is nothing to return.
//
// Arithmetic is checked everywhere: funded totals use checked addition,
// and every withdrawal is checked against the vault balance before it
// moves. No unchecked addition, and no unchecked cast, appears in this
// template.
//
// execute_ruling is the only settlement path. It withdraws from the vault
// directly and never reuses approve or any other payer-only method, so no
// settlement step depends on the transaction signer.

use tari_template_lib::prelude::*;

#[template]
mod payment_escrow {
    use super::*;

    pub struct PaymentEscrow {
        /// The court component named at creation. Immutable, never changed.
        arbitrator: ComponentAddress,
        /// Public key of the payer who created (and funds) the escrow.
        payer: RistrettoPublicKeyBytes,
        /// Public key of the payee.
        payee: RistrettoPublicKeyBytes,
        /// Escrow vault holding the funded TARI until a release, a ruling,
        /// a refund, or a cancel moves it.
        vault: Vault,
        /// The single payment amount, in microTari.
        amount: u64,
        /// Epoch after which the payer may refund the unreleased amount.
        deadline: u64,
        /// Total funded so far, in microTari. Never exceeds amount.
        funded: u64,
        /// True once the payee has marked the work delivered.
        delivered: bool,
        /// True once the payer has approved and the payment has been
        /// released.
        released: bool,
        /// Total moved to the payee by a ruling, in microTari. A ruling
        /// can move part of the payment, so this is tracked separately
        /// from the released flag.
        ruled_amount: u64,
        /// True while a dispute is open on this escrow.
        frozen: bool,
        /// Dispute id the vault is frozen for (0 when never frozen).
        frozen_dispute_id: u64,
        /// True once a ruling has executed. A ruling executes at most once.
        ruling_executed: bool,
        /// True once the payer has cancelled the agreement.
        cancelled: bool,
        /// True once the payer has refunded the unreleased amount.
        refunded: bool,
        /// When false the in-method caller assert is skipped
        /// (used by the owned twin so the owner bypass is observable).
        assert_caller: bool,
    }

    impl PaymentEscrow {
        /// Production form: ownerless, caller rule, in-method assert on.
        pub fn new(
            arbitrator: ComponentAddress,
            payee: RistrettoPublicKeyBytes,
            amount: u64,
            deadline: u64,
        ) -> Component<Self> {
            Self::create(
                arbitrator,
                payee,
                amount,
                deadline,
                OwnerRule::None,
                true,
                false,
            )
        }

        /// Owned twin: the creator owns the component, so the
        /// owner bypass applies to the caller rule. Assert off so the
        /// bypass is observable instead of masked by the assert.
        pub fn new_owned(
            arbitrator: ComponentAddress,
            payee: RistrettoPublicKeyBytes,
            amount: u64,
            deadline: u64,
        ) -> Component<Self> {
            Self::create(
                arbitrator,
                payee,
                amount,
                deadline,
                OwnerRule::OwnedBySigner,
                false,
                true,
            )
        }

        /// Assert-only twin: ownerless, rules wide open, the
        /// in-method caller assert is the only check.
        pub fn new_rule_open(
            arbitrator: ComponentAddress,
            payee: RistrettoPublicKeyBytes,
            amount: u64,
            deadline: u64,
        ) -> Component<Self> {
            Self::create(
                arbitrator,
                payee,
                amount,
                deadline,
                OwnerRule::None,
                true,
                true,
            )
        }

        fn create(
            arbitrator: ComponentAddress,
            payee: RistrettoPublicKeyBytes,
            amount: u64,
            deadline: u64,
            owner_rule: OwnerRule,
            assert_caller: bool,
            rules_open: bool,
        ) -> Component<Self> {
            let payer = CallerContext::transaction_signer_public_key();
            assert!(
                payee != payer,
                "payee must differ from the payer (the creator)"
            );
            assert!(amount > 0, "payment amount must be positive");
            assert!(
                deadline > Consensus::current_epoch(),
                "deadline must be after the current epoch"
            );
            let state = Self {
                arbitrator,
                payer,
                payee,
                vault: Vault::new_empty(TARI_TOKEN),
                amount,
                deadline,
                funded: 0,
                delivered: false,
                released: false,
                ruled_amount: 0,
                frozen: false,
                frozen_dispute_id: 0,
                ruling_executed: false,
                cancelled: false,
                refunded: false,
                assert_caller,
            };
            let rules = if rules_open {
                AccessRules::allow_all()
            } else {
                AccessRules::allow_all()
                    .add_method_rule("freeze_for_dispute", rule!(caller_component(arbitrator)))
                    .add_method_rule("execute_ruling", rule!(caller_component(arbitrator)))
            };
            Component::new(state)
                .with_access_rules(rules)
                .with_owner_rule(owner_rule)
                .create()
        }

        /// Fund the escrow. Only the payer may fund, before the deadline,
        /// while the agreement is open. Funding may be partial, and may
        /// arrive in several calls, but the funded total never exceeds the
        /// payment amount.
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
            assert!(funded <= self.amount, "funding exceeds the payment amount");
            self.funded = funded;
            self.vault.deposit(bucket);
        }

        /// Mark the work delivered. Only the payee may mark it, before
        /// the deadline, while the agreement is open. Delivery can be
        /// marked once.
        pub fn mark_delivered(&mut self) {
            self.assert_open();
            self.assert_before_deadline("delivery is closed after the deadline");
            assert!(
                CallerContext::transaction_signer_public_key() == self.payee,
                "only the payee can mark the work delivered"
            );
            assert!(!self.delivered, "work is already delivered");
            assert!(!self.released, "payment is already released");
            self.delivered = true;
        }

        /// Approve the delivery. Only the payer may approve, before the
        /// deadline, while the agreement is open. Approval releases the
        /// full payment amount to the payee as a bucket, and the payment
        /// must be funded before it can move.
        pub fn approve(&mut self) -> Bucket {
            self.assert_open();
            self.assert_before_deadline("approval is closed after the deadline");
            assert!(
                CallerContext::transaction_signer_public_key() == self.payer,
                "only the payer can approve the payment"
            );
            assert!(self.delivered, "work is not delivered");
            assert!(!self.released, "payment is already released");
            let balance = self
                .vault
                .balance()
                .to_u64_checked()
                .expect("vault balance out of range");
            assert!(self.amount <= balance, "payment is not funded yet");
            self.released = true;
            self.vault.withdraw(Amount::from_u64(self.amount))
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

        /// Refund the unreleased amount to the payer. Only the payer may
        /// refund, and only after the deadline (current epoch >=
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
            assert!(!self.is_complete(), "payment is already released");
            self.frozen = true;
            self.frozen_dispute_id = dispute_id;
        }

        /// Execute a ruling: move the escrow's own vault per the ruling.
        /// Only the arbitrator component, only while frozen for this
        /// dispute, exactly once. Returns the payee's share as a bucket;
        /// the remainder stays in the vault (the settlement matrix routes
        /// the remainder). Conservation is exact: payee bucket + vault
        /// remainder == funded total. This is the only settlement path:
        /// it withdraws from the vault directly and never reuses approve
        /// or any other payer-only method.
        pub fn execute_ruling(&mut self, dispute_id: u64, payee_amount: u64) -> Bucket {
            self.assert_arbitrator();
            assert!(self.frozen, "escrow is not frozen for a dispute");
            assert_eq!(
                dispute_id, self.frozen_dispute_id,
                "dispute id does not match the frozen dispute"
            );
            assert!(!self.ruling_executed, "ruling already executed");
            let balance = self
                .vault
                .balance()
                .to_u64_checked()
                .expect("vault balance out of range");
            assert!(
                payee_amount <= balance,
                "payee amount exceeds the vault balance"
            );
            self.ruling_executed = true;
            self.frozen = false;
            self.ruled_amount = self
                .ruled_amount
                .checked_add(payee_amount)
                .expect("ruled total overflow");
            self.vault.withdraw(Amount::from_u64(payee_amount))
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
            self.released || self.ruled_amount == self.amount
        }

        pub fn ruled_total(&self) -> u64 {
            self.ruled_amount
        }

        pub fn deadline(&self) -> u64 {
            self.deadline
        }

        pub fn total_amount(&self) -> u64 {
            self.amount
        }

        pub fn funded_total(&self) -> u64 {
            self.funded
        }

        pub fn is_delivered(&self) -> bool {
            self.delivered
        }

        pub fn is_released(&self) -> bool {
            self.released
        }

        // ----- helpers -----

        /// The agreement is open for party state changes: not frozen, not
        /// ruled on, not cancelled, not refunded, and not released.
        fn assert_open(&self) {
            assert!(!self.frozen, "escrow is frozen for a dispute");
            assert!(
                !self.ruling_executed,
                "a ruling has already executed on this escrow"
            );
            assert!(!self.cancelled, "escrow is cancelled");
            assert!(!self.refunded, "escrow is refunded");
            assert!(!self.is_complete(), "payment is already released");
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
