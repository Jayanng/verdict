// ArbitratableEscrow, the reference adapter for the Verdict court.
//
// A milestone escrow for freelance work, based on PieceWork, that ships
// with the court built in. The escrow holds its own vault, and its two
// settlement methods accept calls from exactly one component, the arbitrator
// (Verdict court) named at creation. The court never touches the vault; the
// escrow's own code moves it.
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

use tari_template_lib::prelude::*;

#[template]
mod arbitratable_escrow {
    use super::*;

    pub struct ArbitratableEscrow {
        /// The court component named at creation. Immutable, never changed.
        arbitrator: ComponentAddress,
        /// Public key of the payer who created (and funds) the escrow.
        payer: RistrettoPublicKeyBytes,
        /// Public key of the payee.
        payee: RistrettoPublicKeyBytes,
        /// Escrow vault holding the funded TARI until a ruling moves it.
        vault: Vault,
        /// True while a dispute is open on this escrow.
        frozen: bool,
        /// Dispute id the vault is frozen for (0 when never frozen).
        frozen_dispute_id: u64,
        /// True once a ruling has executed. A ruling executes at most once.
        ruling_executed: bool,
        /// When false the in-method caller assert is skipped
        /// (used by the owned twin so the owner bypass is observable).
        assert_caller: bool,
    }

    impl ArbitratableEscrow {
        /// Production form: ownerless, caller rule, in-method assert on.
        pub fn new(
            arbitrator: ComponentAddress,
            payee: RistrettoPublicKeyBytes,
        ) -> Component<Self> {
            Self::create(arbitrator, payee, OwnerRule::None, true, false)
        }

        /// Owned twin: the creator owns the component, so the
        /// owner bypass applies to the caller rule. Assert off so the
        /// bypass is observable instead of masked by the assert.
        pub fn new_owned(
            arbitrator: ComponentAddress,
            payee: RistrettoPublicKeyBytes,
        ) -> Component<Self> {
            Self::create(arbitrator, payee, OwnerRule::OwnedBySigner, false, true)
        }

        /// Assert-only twin: ownerless, rules wide open, the
        /// in-method caller assert is the only check.
        pub fn new_rule_open(
            arbitrator: ComponentAddress,
            payee: RistrettoPublicKeyBytes,
        ) -> Component<Self> {
            Self::create(arbitrator, payee, OwnerRule::None, true, true)
        }

        fn create(
            arbitrator: ComponentAddress,
            payee: RistrettoPublicKeyBytes,
            owner_rule: OwnerRule,
            assert_caller: bool,
            rules_open: bool,
        ) -> Component<Self> {
            let payer = CallerContext::transaction_signer_public_key();
            assert!(
                payee != payer,
                "payee must differ from the payer (the creator)"
            );
            let state = Self {
                arbitrator,
                payer,
                payee,
                vault: Vault::new_empty(TARI_TOKEN),
                frozen: false,
                frozen_dispute_id: 0,
                ruling_executed: false,
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

        /// Fund the escrow. Only the payer may fund.
        pub fn fund(&mut self, bucket: Bucket) {
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
            self.vault.deposit(bucket);
        }

        /// Freeze the vault for a dispute. Only the arbitrator component.
        pub fn freeze_for_dispute(&mut self, dispute_id: u64) {
            self.assert_arbitrator();
            assert!(!self.frozen, "escrow is already frozen for a dispute");
            assert!(
                !self.ruling_executed,
                "a ruling has already executed on this escrow"
            );
            self.frozen = true;
            self.frozen_dispute_id = dispute_id;
        }

        /// Execute a ruling: move the escrow's own vault per the ruling.
        /// Only the arbitrator component, only while frozen for this
        /// dispute, exactly once. Returns the payee's share as a bucket;
        /// the remainder stays in the vault (the settlement matrix, which
        /// routes the remainder, is built in the next phase). Conservation
        /// is exact: payee bucket + vault remainder == funded total.
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

        // ----- helpers -----

        fn assert_arbitrator(&self) {
            if self.assert_caller {
                CallerContext::assert_badge_in_scope(&NonFungibleAddress::caller_component_badge(
                    self.arbitrator,
                ));
            }
        }
    }
}
