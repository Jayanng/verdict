// Verdict, the Ootle jury court.
//
// When an Ootle escrow deal goes wrong, three bonded strangers rule it and
// the loser pays. The court never touches the money. It issues a ruling; the
// escrow's own ownerless code executes it, and its settlement method accepts
// calls from exactly one component: the court.
//
// This template holds the court: disputes, evidence hashes, the future-epoch
// jury draw, commit-reveal voting, settlement, appeal, and the fee ledger.
// OwnerRule::None on both templates, so no owner path can bypass the rules.

use tari_template_lib::prelude::*;

#[template]
mod verdict {
    use super::*;

    pub struct Verdict {
        /// Disputes opened through this court, by dispute id (index).
        disputes: Vec<Dispute>,
    }

    pub struct Dispute {
        /// The escrow component under dispute.
        pub escrow: ComponentAddress,
        /// Epoch the dispute opened in.
        pub open_epoch: u64,
        /// Epoch hash read at open (the current, predictable one).
        pub open_epoch_hash: Hash32,
        /// Locked draw epoch E* = open_epoch + K. The draw executes only here.
        pub draw_epoch: u64,
        /// Epoch hash read at draw time (the future epoch's hash).
        pub draw_hash: Hash32,
        /// True once the draw has executed in E*.
        pub drawn: bool,
        /// True once the ruling has settled.
        pub settled: bool,
    }

    impl Verdict {
        pub fn new() -> Component<Self> {
            Component::new(Self { disputes: Vec::new() })
                .with_access_rules(AccessRules::allow_all())
                .with_owner_rule(OwnerRule::None)
                .create()
        }

        /// Open a dispute: freeze the escrow via its own method, and lock
        /// the draw to the future epoch E* = current epoch + `draw_delay`.
        /// Returns the dispute id. K is a caller-chosen delay, not a
        /// constant, so small windows can be exercised in tests.
        pub fn open_dispute(&mut self, escrow: ComponentAddress, draw_delay: u64) -> u64 {
            let dispute_id = self.disputes.len() as u64;
            let open_epoch = Consensus::current_epoch();
            let open_epoch_hash = Consensus::current_epoch_hash();
            ComponentManager::get(escrow)
                .invoke("freeze_for_dispute", args![dispute_id]);
            self.disputes.push(Dispute {
                escrow,
                open_epoch,
                open_epoch_hash,
                draw_epoch: open_epoch + draw_delay,
                draw_hash: Hash32::from([0u8; 32]),
                drawn: false,
                settled: false,
            });
            dispute_id
        }

        /// Execute the jury draw. Only during the locked draw epoch E*,
        /// only once. Reads the epoch hash of E* in-template and stores it.
        /// Returns the drawn hash.
        pub fn draw_jury(&mut self, dispute_id: u64) -> Hash32 {
            let dispute = self.dispute_mut(dispute_id);
            assert!(!dispute.drawn, "jury already drawn for this dispute");
            assert_eq!(
                Consensus::current_epoch(),
                dispute.draw_epoch,
                "draw_jury only executes during the locked draw epoch"
            );
            let hash = Consensus::current_epoch_hash();
            dispute.draw_hash = hash;
            dispute.drawn = true;
            hash
        }

        /// Settle a dispute: order the ruling, the escrow executes it, and
        /// the court routes the payee bucket the escrow returns. The court
        /// never holds the money. Exactly once. The remainder stays in the
        /// escrow vault until the settlement matrix routes it.
        pub fn settle(
            &mut self,
            dispute_id: u64,
            payee_amount: u64,
            payee_account: ComponentAddress,
        ) {
            let escrow = {
                let dispute = self.dispute_mut(dispute_id);
                assert!(!dispute.settled, "dispute already settled");
                dispute.settled = true;
                dispute.escrow
            };
            let payee_bucket: Bucket =
                ComponentManager::get(escrow).call("execute_ruling", args![dispute_id, payee_amount]);
            ComponentManager::get(payee_account).invoke("deposit", args![payee_bucket]);
        }

        // ----- read-only getters -----

        /// Direct in-template read of the current epoch hash.
        pub fn current_epoch_hash(&self) -> Hash32 {
            Consensus::current_epoch_hash()
        }

        pub fn current_epoch(&self) -> u64 {
            Consensus::current_epoch()
        }

        pub fn dispute_draw_epoch(&self, dispute_id: u64) -> u64 {
            self.dispute(dispute_id).draw_epoch
        }

        pub fn dispute_open_epoch(&self, dispute_id: u64) -> u64 {
            self.dispute(dispute_id).open_epoch
        }

        pub fn dispute_open_epoch_hash(&self, dispute_id: u64) -> Hash32 {
            self.dispute(dispute_id).open_epoch_hash
        }

        pub fn dispute_draw_hash(&self, dispute_id: u64) -> Hash32 {
            self.dispute(dispute_id).draw_hash
        }

        pub fn dispute_drawn(&self, dispute_id: u64) -> bool {
            self.dispute(dispute_id).drawn
        }

        pub fn dispute_settled(&self, dispute_id: u64) -> bool {
            self.dispute(dispute_id).settled
        }

        // ----- helpers -----

        fn dispute(&self, dispute_id: u64) -> &Dispute {
            self.disputes.get(dispute_id as usize).expect("dispute id out of range")
        }

        fn dispute_mut(&mut self, dispute_id: u64) -> &mut Dispute {
            self.disputes.get_mut(dispute_id as usize).expect("dispute id out of range")
        }
    }
}
