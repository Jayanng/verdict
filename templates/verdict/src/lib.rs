// Verdict, the Ootle jury court.
//
// When an Ootle escrow deal goes wrong, three bonded strangers rule it and
// the loser pays. The court never touches the money. It issues a ruling; the
// escrow's own ownerless code executes it, and its settlement method accepts
// calls from exactly one component: the court.
//
// This template holds the court: the juror pool, disputes, evidence hashes,
// the future-epoch jury draw, commit-reveal voting, settlement, appeal, and
// the fee ledger. OwnerRule::None on both templates, so no owner path can
// bypass the rules.

use tari_template_lib::prelude::*;

#[template]
mod verdict {
    use super::*;

    /// Jurors drawn for one ruling.
    const PANEL_SIZE: u64 = 3;
    /// Fee paid to each juror for one dispute, in micro-Tari.
    const JUROR_FEE: u64 = 5_000;
    /// Bond floor: at least twice the largest juror fee times the panel.
    const BOND_FLOOR: u64 = 2 * JUROR_FEE * PANEL_SIZE;
    /// Evidence items allowed per side of a dispute.
    const EVIDENCE_LIMIT: u64 = 3;

    /// Votes a juror can cast.
    const VOTE_PAY: u64 = 0;
    const VOTE_REFUND: u64 = 1;
    const VOTE_SPLIT: u64 = 2;
    const VOTE_REFUSE: u64 = 3;

    pub struct Verdict {
        /// Disputes opened through this court, by dispute id (index).
        disputes: Vec<Dispute>,
        /// Registered jurors, in registration order.
        jurors: Vec<Juror>,
        /// Vault holding juror bonds.
        bond_vault: Vault,
    }

    pub struct Juror {
        /// Public key of the juror (the transaction signer at registration).
        pub key: RistrettoPublicKeyBytes,
        /// Bond posted, in micro-Tari.
        pub bond: u64,
    }

    pub struct EvidenceItem {
        /// SHA-256 of the evidence bytes.
        pub sha256: Hash32,
        /// Where the evidence bytes live.
        pub uri: String,
        /// Length of the evidence bytes.
        pub bytes_len: u64,
    }

    pub struct VoteCommit {
        /// Public key of the committing juror.
        pub juror: RistrettoPublicKeyBytes,
        /// Commitment hash of (vote, salt, dispute id, juror).
        pub commitment: Hash32,
    }

    pub struct VoteReveal {
        /// Public key of the revealing juror.
        pub juror: RistrettoPublicKeyBytes,
        /// The revealed vote.
        pub vote: u64,
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
        /// Draw round nonce, 0 for the first draw.
        pub round: u64,
        /// Juror keys eligible for this dispute, in registration order.
        /// Fixed when the dispute opens, so later registrations cannot
        /// change the drawn panel.
        pub eligible_pool: Vec<RistrettoPublicKeyBytes>,
        /// Jurors drawn for this dispute, by public key.
        pub panel: Vec<RistrettoPublicKeyBytes>,
        /// Evidence posted by the payer, by item index (0..3).
        pub payer_evidence: [Option<EvidenceItem>; EVIDENCE_LIMIT as usize],
        /// Evidence posted by the payee, by item index (0..3).
        pub payee_evidence: [Option<EvidenceItem>; EVIDENCE_LIMIT as usize],
        /// Vote commitments, one per juror.
        pub commits: Vec<VoteCommit>,
        /// Revealed votes, one per juror.
        pub reveals: Vec<VoteReveal>,
    }

    impl Verdict {
        pub fn new() -> Component<Self> {
            Component::new(Self {
                disputes: Vec::new(),
                jurors: Vec::new(),
                bond_vault: Vault::new_empty(TARI_TOKEN),
            })
            .with_access_rules(AccessRules::allow_all())
            .with_owner_rule(OwnerRule::None)
            .create()
        }

        /// Register as a juror by posting a bond. One slot per address.
        /// The bond must reach the floor: twice the largest juror fee
        /// times the panel size.
        pub fn register_juror(&mut self, bond: Bucket) {
            let signer = CallerContext::transaction_signer_public_key();
            assert!(
                !self.jurors.iter().any(|j| j.key == signer),
                "juror already registered"
            );
            assert!(
                bond.resource_address() == TARI_TOKEN,
                "juror bond must be TARI"
            );
            let amount = bond
                .amount()
                .to_u64_checked()
                .expect("bond amount out of range");
            assert!(amount >= BOND_FLOOR, "bond below the floor");
            self.bond_vault.deposit(bond);
            self.jurors.push(Juror {
                key: signer,
                bond: amount,
            });
        }

        /// Open a dispute: freeze the escrow via its own method, and lock
        /// the draw to the future epoch E* = current epoch + `draw_delay`.
        /// Returns the dispute id. K is a caller-chosen delay, not a
        /// constant, so small windows can be exercised in tests.
        pub fn open_dispute(&mut self, escrow: ComponentAddress, draw_delay: u64) -> u64 {
            let dispute_id = self.disputes.len() as u64;
            let open_epoch = Consensus::current_epoch();
            let open_epoch_hash = Consensus::current_epoch_hash();
            let draw_epoch = open_epoch
                .checked_add(draw_delay)
                .expect("draw delay overflows epoch range");
            // Freeze the eligible pool now: jurors who register after
            // the dispute opens are not eligible for this dispute.
            let eligible_pool: Vec<RistrettoPublicKeyBytes> =
                self.jurors.iter().map(|j| j.key).collect();
            ComponentManager::get(escrow).invoke("freeze_for_dispute", args![dispute_id]);
            self.disputes.push(Dispute {
                escrow,
                open_epoch,
                open_epoch_hash,
                draw_epoch,
                draw_hash: Hash32::from([0u8; 32]),
                drawn: false,
                settled: false,
                round: 0,
                eligible_pool,
                panel: Vec::new(),
                payer_evidence: [None, None, None],
                payee_evidence: [None, None, None],
                commits: Vec::new(),
                reveals: Vec::new(),
            });
            dispute_id
        }

        /// Submit one evidence item for the caller's side of a dispute.
        /// At most 3 items per side, by index 0, 1, 2. An item, once
        /// posted, cannot be changed: posting to a filled index fails.
        pub fn submit_evidence(
            &mut self,
            dispute_id: u64,
            index: u64,
            sha256: Hash32,
            uri: String,
            bytes_len: u64,
        ) {
            assert!(index < EVIDENCE_LIMIT, "evidence index out of range");
            let escrow = self.dispute(dispute_id).escrow;
            let payer: RistrettoPublicKeyBytes =
                ComponentManager::get(escrow).call("payer", args![]);
            let payee: RistrettoPublicKeyBytes =
                ComponentManager::get(escrow).call("payee", args![]);
            let signer = CallerContext::transaction_signer_public_key();
            let is_payer = signer == payer;
            let is_payee = signer == payee;
            assert!(
                is_payer || is_payee,
                "only a party to the dispute can submit evidence"
            );
            let item = EvidenceItem {
                sha256,
                uri,
                bytes_len,
            };
            let dispute = self.dispute_mut(dispute_id);
            let slot = if is_payer {
                &mut dispute.payer_evidence[index as usize]
            } else {
                &mut dispute.payee_evidence[index as usize]
            };
            assert!(slot.is_none(), "evidence already posted at this index");
            *slot = Some(item);
        }

        /// Execute the jury draw. Only during the locked draw epoch E*,
        /// only once. Reads the epoch hash of E* in-template, derives the
        /// panel from the frozen eligible pool, and stores both. Returns
        /// the drawn hash.
        pub fn draw_jury(&mut self, dispute_id: u64) -> Hash32 {
            {
                let dispute = self.dispute(dispute_id);
                assert!(!dispute.drawn, "jury already drawn for this dispute");
                assert_eq!(
                    Consensus::current_epoch(),
                    dispute.draw_epoch,
                    "draw_jury only executes during the locked draw epoch"
                );
            }
            let hash = Consensus::current_epoch_hash();
            let panel = {
                let dispute = self.dispute(dispute_id);
                Self::derive_panel(&dispute.eligible_pool, hash, dispute_id, dispute.round)
            };
            let dispute = self.dispute_mut(dispute_id);
            dispute.draw_hash = hash;
            dispute.drawn = true;
            dispute.panel = panel;
            hash
        }

        /// Commit a vote: post the hash of (vote, salt, dispute id,
        /// juror). Only a drawn juror can commit, and only once.
        pub fn commit_vote(&mut self, dispute_id: u64, commitment: Hash32) {
            let signer = CallerContext::transaction_signer_public_key();
            let dispute = self.dispute_mut(dispute_id);
            assert!(dispute.drawn, "jury not drawn for this dispute");
            assert!(
                dispute.panel.contains(&signer),
                "juror not drawn for this dispute"
            );
            assert!(
                !dispute.commits.iter().any(|c| c.juror == signer),
                "juror already committed a vote"
            );
            dispute.commits.push(VoteCommit {
                juror: signer,
                commitment,
            });
        }

        /// Reveal a vote: post the vote and salt. The court recomputes
        /// the commitment and it must match. Only once per juror.
        pub fn reveal_vote(&mut self, dispute_id: u64, vote: u64, salt: Hash32) {
            let signer = CallerContext::transaction_signer_public_key();
            assert!(vote <= VOTE_REFUSE, "invalid vote");
            let expected = Self::vote_commitment(vote, salt, dispute_id, signer);
            let dispute = self.dispute_mut(dispute_id);
            let stored = dispute
                .commits
                .iter()
                .find(|c| c.juror == signer)
                .map(|c| c.commitment)
                .expect("no vote commitment from this juror");
            assert!(stored == expected, "reveal does not match the commitment");
            assert!(
                !dispute.reveals.iter().any(|r| r.juror == signer),
                "juror already revealed a vote"
            );
            dispute.reveals.push(VoteReveal {
                juror: signer,
                vote,
            });
        }

        /// Settle a dispute: order the ruling, the escrow executes it, and
        /// the court routes both buckets the escrow returns. The court
        /// never holds the money. Exactly once. The payee bucket goes to
        /// the payee account, and the remainder bucket goes straight to
        /// the payer account, so a split or a refund never leaves funds
        /// locked in the vault.
        pub fn settle(
            &mut self,
            dispute_id: u64,
            payee_amount: u64,
            payee_account: ComponentAddress,
            payer_account: ComponentAddress,
        ) {
            let escrow = {
                let dispute = self.dispute_mut(dispute_id);
                assert!(!dispute.settled, "dispute already settled");
                dispute.settled = true;
                dispute.escrow
            };
            let payee_bucket: Bucket = ComponentManager::get(escrow)
                .call("execute_ruling", args![dispute_id, payee_amount]);
            ComponentManager::get(payee_account).invoke("deposit", args![payee_bucket]);
            let remainder_bucket: Bucket =
                ComponentManager::get(escrow).call("withdraw_remainder", args![]);
            ComponentManager::get(payer_account).invoke("deposit", args![remainder_bucket]);
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

        /// Draw round of a dispute. 0 for the first draw. Published so
        /// an external verifier can recompute the panel from the draw
        /// inputs: draw hash, dispute id, round, and the frozen pool.
        pub fn dispute_round(&self, dispute_id: u64) -> u64 {
            self.dispute(dispute_id).round
        }

        /// Length of the frozen eligible pool for a dispute.
        pub fn dispute_eligible_pool_len(&self, dispute_id: u64) -> u64 {
            self.dispute(dispute_id).eligible_pool.len() as u64
        }

        /// One juror key from the frozen eligible pool, by index in
        /// registration order. Published so an external verifier can
        /// recompute the panel from the draw inputs.
        pub fn dispute_eligible_pool_member(
            &self,
            dispute_id: u64,
            index: u64,
        ) -> RistrettoPublicKeyBytes {
            let dispute = self.dispute(dispute_id);
            assert!(
                index < dispute.eligible_pool.len() as u64,
                "eligible pool index out of range"
            );
            dispute.eligible_pool[index as usize]
        }

        pub fn juror_count(&self) -> u64 {
            self.jurors.len() as u64
        }

        pub fn is_juror(&self, key: RistrettoPublicKeyBytes) -> bool {
            self.jurors.iter().any(|j| j.key == key)
        }

        pub fn juror_bond(&self, key: RistrettoPublicKeyBytes) -> u64 {
            self.jurors
                .iter()
                .find(|j| j.key == key)
                .map(|j| j.bond)
                .unwrap_or(0)
        }

        pub fn bond_floor(&self) -> u64 {
            BOND_FLOOR
        }

        pub fn dispute_panel_len(&self, dispute_id: u64) -> u64 {
            self.dispute(dispute_id).panel.len() as u64
        }

        pub fn dispute_panel_member(&self, dispute_id: u64, index: u64) -> RistrettoPublicKeyBytes {
            let dispute = self.dispute(dispute_id);
            assert!(
                index < dispute.panel.len() as u64,
                "panel index out of range"
            );
            dispute.panel[index as usize]
        }

        /// Evidence count for one side of a dispute. Side 0 is the payer,
        /// side 1 is the payee.
        pub fn dispute_evidence_count(&self, dispute_id: u64, side: u64) -> u64 {
            assert!(side <= 1, "invalid evidence side");
            let dispute = self.dispute(dispute_id);
            let slots = if side == 0 {
                &dispute.payer_evidence
            } else {
                &dispute.payee_evidence
            };
            slots.iter().filter(|s| s.is_some()).count() as u64
        }

        /// SHA-256 of one posted evidence item. Side 0 is the payer,
        /// side 1 is the payee.
        pub fn dispute_evidence_hash(&self, dispute_id: u64, side: u64, index: u64) -> Hash32 {
            assert!(side <= 1, "invalid evidence side");
            assert!(index < EVIDENCE_LIMIT, "evidence index out of range");
            let dispute = self.dispute(dispute_id);
            let slots = if side == 0 {
                &dispute.payer_evidence
            } else {
                &dispute.payee_evidence
            };
            slots[index as usize]
                .as_ref()
                .expect("no evidence at this index")
                .sha256
        }

        pub fn dispute_commit_count(&self, dispute_id: u64) -> u64 {
            self.dispute(dispute_id).commits.len() as u64
        }

        pub fn dispute_reveal_count(&self, dispute_id: u64) -> u64 {
            self.dispute(dispute_id).reveals.len() as u64
        }

        pub fn dispute_reveal(&self, dispute_id: u64, juror: RistrettoPublicKeyBytes) -> u64 {
            self.dispute(dispute_id)
                .reveals
                .iter()
                .find(|r| r.juror == juror)
                .map(|r| r.vote)
                .expect("juror has not revealed a vote")
        }

        // ----- helpers -----

        fn dispute(&self, dispute_id: u64) -> &Dispute {
            self.disputes
                .get(dispute_id as usize)
                .expect("dispute id out of range")
        }

        fn dispute_mut(&mut self, dispute_id: u64) -> &mut Dispute {
            self.disputes
                .get_mut(dispute_id as usize)
                .expect("dispute id out of range")
        }

        /// The vote commitment: SHA-256 of (vote, salt, dispute id,
        /// juror), each part in little-endian or raw bytes, concatenated
        /// in order.
        fn vote_commitment(
            vote: u64,
            salt: Hash32,
            dispute_id: u64,
            juror: RistrettoPublicKeyBytes,
        ) -> Hash32 {
            intrinsics::hash_sha256_parts(&[
                &vote.to_le_bytes(),
                salt.as_ref(),
                &dispute_id.to_le_bytes(),
                juror.as_ref(),
            ])
        }

        /// Derive the panel from the drawn hash, the dispute id, the draw
        /// round, and the frozen eligible pool. For each slot, read an
        /// 8-byte window of the hash, fold in the dispute id, the round,
        /// and the slot, and take the result modulo the pool size,
        /// probing forward until an unpicked juror is found. Returns at
        /// most PANEL_SIZE jurors, fewer when the pool is smaller.
        fn derive_panel(
            pool: &[RistrettoPublicKeyBytes],
            draw_hash: Hash32,
            dispute_id: u64,
            round: u64,
        ) -> Vec<RistrettoPublicKeyBytes> {
            let pool_len = pool.len() as u64;
            if pool_len == 0 {
                return Vec::new();
            }
            let target = PANEL_SIZE.min(pool_len) as usize;
            let hash_bytes: &[u8] = draw_hash.as_ref();
            let mut panel: Vec<RistrettoPublicKeyBytes> = Vec::new();
            for slot in 0..target as u64 {
                let offset = ((slot * 8) % 32) as usize;
                let mut bytes = [0u8; 8];
                for i in 0..8 {
                    bytes[i] = hash_bytes[(offset + i) % 32];
                }
                let value = u64::from_le_bytes(bytes);
                let mut candidate = (value
                    .wrapping_add(dispute_id)
                    .wrapping_add(round)
                    .wrapping_add(slot))
                    % pool_len;
                while panel.contains(&pool[candidate as usize]) {
                    candidate = (candidate + 1) % pool_len;
                }
                panel.push(pool[candidate as usize]);
            }
            panel
        }
    }
}
