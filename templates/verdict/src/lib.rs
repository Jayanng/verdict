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
    /// Jurors drawn for an appeal ruling.
    const APPEAL_PANEL_SIZE: u64 = 7;
    /// Fee paid to each juror for one dispute, in micro-Tari.
    const JUROR_FEE: u64 = 5_000;
    /// Bond floor: at least twice the largest juror fee times the panel.
    const BOND_FLOOR: u64 = 2 * JUROR_FEE * PANEL_SIZE;
    /// Fee an appellant posts to open an appeal. It funds the appeal
    /// jurors, one juror fee each, and the unspent part returns to the
    /// appellant when the dispute settles.
    const APPEAL_FEE: u64 = APPEAL_PANEL_SIZE * JUROR_FEE;
    /// Bond forfeited by one juror who votes against the majority, or who
    /// is drawn and does not reveal. The forfeited bonds top up the
    /// jurors who carried the ruling.
    const SLASH: u64 = JUROR_FEE;
    /// Evidence items allowed per side of a dispute.
    const EVIDENCE_LIMIT: u64 = 3;
    /// Outcome value meaning no ruling has been reached yet.
    const NO_OUTCOME: u64 = 4;

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
        /// Vault holding juror fees taken from the principal at settle,
        /// appeal fees posted by appellants, and slashed bond funds,
        /// until jurors claim them with claim_fee.
        fee_vault: Vault,
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

    pub struct FeeClaim {
        /// Public key of the juror the fee is owed to.
        pub juror: RistrettoPublicKeyBytes,
        /// Amount owed, in micro-Tari: the juror fee plus any slashed
        /// bond share. Zero means no fee is owed.
        pub amount: u64,
        /// True once the juror has claimed the fee.
        pub claimed: bool,
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
        /// True once the dispute has closed with no ruling, under the
        /// bounded fallback. A closed dispute can never settle.
        pub closed_no_ruling: bool,
        /// Ruling outcome once settled: VOTE_PAY, VOTE_REFUND, or
        /// VOTE_SPLIT. NO_OUTCOME until the dispute settles.
        pub outcome: u64,
        /// Amount the ruling paid to the payee, in micro-Tari.
        pub payee_amount: u64,
        /// Juror fees taken from the principal at settle, in micro-Tari.
        pub fees_paid: u64,
        /// Draw round nonce, 0 for the first draw.
        pub round: u64,
        /// True once the one replacement draw has been used.
        pub replacement_used: bool,
        /// Times the draw was rescheduled after its locked epoch passed
        /// without a draw.
        pub reschedules: u64,
        /// Drawn jurors who failed to reveal in a replaced round. They
        /// are slashed when the dispute settles or closes.
        pub failed_jurors: Vec<RistrettoPublicKeyBytes>,
        /// Fee claims recorded at settle or close, one per juror owed.
        pub claims: Vec<FeeClaim>,
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
        /// True once an appeal has been opened on this dispute.
        pub appealed: bool,
        /// Public key of the appellant, the party who opened the appeal.
        pub appellant: Option<RistrettoPublicKeyBytes>,
        /// Account the unspent appeal fee returns to at settle or close.
        pub appellant_account: Option<ComponentAddress>,
        /// Appeal fee posted by the appellant, in micro-Tari.
        pub appeal_fee: u64,
        /// Locked appeal draw epoch E_appeal = appeal epoch + K2.
        pub appeal_draw_epoch: u64,
        /// Epoch hash read at appeal draw time.
        pub appeal_draw_hash: Hash32,
        /// True once the appeal draw has executed.
        pub appeal_drawn: bool,
        /// True once the one appeal replacement draw has been used.
        pub appeal_replacement_used: bool,
        /// Appeal draw round nonce, 0 for the first appeal draw.
        pub appeal_round: u64,
        /// Times the appeal draw was rescheduled after its locked epoch
        /// passed without a draw.
        pub appeal_reschedules: u64,
        /// Jurors drawn for the appeal, by public key.
        pub appeal_panel: Vec<RistrettoPublicKeyBytes>,
        /// Appeal vote commitments, one per juror.
        pub appeal_commits: Vec<VoteCommit>,
        /// Appeal revealed votes, one per juror.
        pub appeal_reveals: Vec<VoteReveal>,
    }

    impl Verdict {
        pub fn new() -> Component<Self> {
            Component::new(Self {
                disputes: Vec::new(),
                jurors: Vec::new(),
                bond_vault: Vault::new_empty(TARI_TOKEN),
                fee_vault: Vault::new_empty(TARI_TOKEN),
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
                closed_no_ruling: false,
                outcome: NO_OUTCOME,
                payee_amount: 0,
                fees_paid: 0,
                round: 0,
                replacement_used: false,
                reschedules: 0,
                failed_jurors: Vec::new(),
                claims: Vec::new(),
                eligible_pool,
                panel: Vec::new(),
                payer_evidence: [None, None, None],
                payee_evidence: [None, None, None],
                commits: Vec::new(),
                reveals: Vec::new(),
                appealed: false,
                appellant: None,
                appellant_account: None,
                appeal_fee: 0,
                appeal_draw_epoch: 0,
                appeal_draw_hash: Hash32::from([0u8; 32]),
                appeal_drawn: false,
                appeal_replacement_used: false,
                appeal_round: 0,
                appeal_reschedules: 0,
                appeal_panel: Vec::new(),
                appeal_commits: Vec::new(),
                appeal_reveals: Vec::new(),
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
                Self::derive_panel(
                    &dispute.eligible_pool,
                    hash,
                    dispute_id,
                    dispute.round,
                    PANEL_SIZE,
                )
            };
            let dispute = self.dispute_mut(dispute_id);
            dispute.draw_hash = hash;
            dispute.drawn = true;
            dispute.panel = panel;
            hash
        }

        /// Reschedule a missed draw: if the locked draw epoch has passed
        /// without a draw, move the commitment to the next epoch. The
        /// reschedule is recorded on chain, so it cannot happen quietly.
        /// Callable as often as the window keeps being missed.
        pub fn reschedule_draw(&mut self, dispute_id: u64) {
            let dispute = self.dispute_mut(dispute_id);
            assert!(!dispute.drawn, "jury already drawn for this dispute");
            assert!(!dispute.settled, "dispute already settled");
            assert!(!dispute.closed_no_ruling, "dispute closed with no ruling");
            assert!(
                Consensus::current_epoch() > dispute.draw_epoch,
                "draw epoch has not passed yet"
            );
            dispute.draw_epoch = dispute
                .draw_epoch
                .checked_add(1)
                .expect("draw epoch overflow");
            dispute.reschedules = dispute
                .reschedules
                .checked_add(1)
                .expect("reschedule count overflow");
        }

        /// Record a failed round and commit a new future epoch for the
        /// replacement draw. Allowed once per dispute, only when the drawn
        /// round cannot produce a ruling: some drawn juror has not
        /// revealed, and no outcome holds a majority of the reveals. The
        /// jurors who did not reveal are recorded as failed; they are
        /// slashed when the dispute settles or closes. The panel,
        /// commits, and reveals are cleared, the round nonce advances,
        /// and the next draw executes only in the new locked epoch.
        pub fn replace_jury(&mut self, dispute_id: u64, draw_delay: u64) {
            {
                let dispute = self.dispute(dispute_id);
                assert!(dispute.drawn, "jury not drawn for this dispute");
                assert!(!dispute.settled, "dispute already settled");
                assert!(!dispute.closed_no_ruling, "dispute closed with no ruling");
                assert!(!dispute.appealed, "dispute is under appeal");
                assert!(!dispute.replacement_used, "replacement already used");
                assert!(
                    dispute.reveals.len() < dispute.panel.len(),
                    "round is fully revealed"
                );
                assert!(
                    Self::majority_outcome(&dispute.reveals, dispute.panel.len() as u64).is_none(),
                    "round already has a majority ruling"
                );
            }
            let now = Consensus::current_epoch();
            let dispute = self.dispute_mut(dispute_id);
            for juror in dispute.panel.clone() {
                if !dispute.reveals.iter().any(|r| r.juror == juror)
                    && !dispute.failed_jurors.contains(&juror)
                {
                    dispute.failed_jurors.push(juror);
                }
            }
            dispute.draw_epoch = now
                .checked_add(draw_delay)
                .expect("draw delay overflows epoch range");
            dispute.draw_hash = Hash32::from([0u8; 32]);
            dispute.drawn = false;
            dispute.round = dispute.round.checked_add(1).expect("draw round overflow");
            dispute.panel = Vec::new();
            dispute.commits = Vec::new();
            dispute.reveals = Vec::new();
            dispute.replacement_used = true;
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

        /// Settle a dispute: the ruling is the majority of the revealed
        /// votes, never a caller-supplied amount. The court reads the
        /// escrow vault balance as the principal, works out the payee
        /// amount from the ruling outcome, orders the ruling, and routes
        /// both buckets the escrow returns. Jurors are paid first: the
        /// fees owed to the majority jurors are taken from the remainder
        /// bucket into the fee vault before the payer is paid. Costs are
        /// capped at the principal. Exactly once. When the dispute is
        /// under appeal, the appeal ruling settles it, the appeal jurors
        /// are paid from the appeal fee, and the unspent appeal fee
        /// returns to the appellant. The court never holds the money
        /// beyond the fee vault.
        pub fn settle(
            &mut self,
            dispute_id: u64,
            payee_account: ComponentAddress,
            payer_account: ComponentAddress,
        ) {
            // Read the ruling and the principal before any state
            // changes, so a failed settle leaves the dispute untouched.
            let (escrow, outcome, first_outcome, principal) = {
                let dispute = self.dispute(dispute_id);
                assert!(!dispute.settled, "dispute already settled");
                assert!(!dispute.closed_no_ruling, "dispute closed with no ruling");
                let first_outcome =
                    Self::majority_outcome(&dispute.reveals, dispute.panel.len() as u64)
                        .expect("first round has no majority ruling");
                let outcome = if dispute.appealed {
                    Self::majority_outcome(
                        &dispute.appeal_reveals,
                        dispute.appeal_panel.len() as u64,
                    )
                    .expect("appeal round has no majority ruling")
                } else {
                    first_outcome
                };
                let escrow = dispute.escrow;
                let principal: u64 = ComponentManager::get(escrow).call("vault_balance", args![]);
                (escrow, outcome, first_outcome, principal)
            };
            // Fees owed from the principal: one juror fee per juror in
            // the first-round majority, capped at the principal. When
            // the dispute is under appeal, the appeal jurors are paid
            // from the appeal fee instead, so the principal funds only
            // the first-round majority.
            let (panel, reveals, appeal_panel, appeal_reveals, failed_jurors) = {
                let dispute = self.dispute(dispute_id);
                (
                    dispute.panel.clone(),
                    dispute
                        .reveals
                        .iter()
                        .map(|r| VoteReveal {
                            juror: r.juror,
                            vote: r.vote,
                        })
                        .collect::<Vec<VoteReveal>>(),
                    dispute.appeal_panel.clone(),
                    dispute
                        .appeal_reveals
                        .iter()
                        .map(|r| VoteReveal {
                            juror: r.juror,
                            vote: r.vote,
                        })
                        .collect::<Vec<VoteReveal>>(),
                    dispute.failed_jurors.clone(),
                )
            };
            let first_majority_keys = Self::majority_keys(&panel, &reveals, first_outcome);
            let first_majority = first_majority_keys.len() as u64;
            let fees = first_majority
                .checked_mul(JUROR_FEE)
                .expect("juror fee total overflow")
                .min(principal);
            let payee_amount = match outcome {
                VOTE_PAY => principal.checked_sub(fees).expect("payee amount underflow"),
                VOTE_REFUND => 0,
                _ => {
                    // Split: half of the post-fee principal to the payee,
                    // rounded up, so the odd micro-Tari goes to the payee
                    // and the payer keeps the floor half.
                    let net = principal.checked_sub(fees).expect("split amount underflow");
                    net.checked_add(1)
                        .expect("split amount overflow")
                        .checked_div(2)
                        .expect("split division by zero")
                }
            };
            assert!(
                payee_amount <= principal,
                "payee amount exceeds the principal"
            );
            // Order the ruling: the escrow moves its own vault exactly
            // once, then returns the remainder.
            let payee_bucket: Bucket = ComponentManager::get(escrow)
                .call("execute_ruling", args![dispute_id, payee_amount]);
            let mut remainder_bucket: Bucket =
                ComponentManager::get(escrow).call("withdraw_remainder", args![]);
            let payee_moved = payee_bucket
                .amount()
                .to_u64_checked()
                .expect("payee bucket amount out of range");
            let remainder_moved = remainder_bucket
                .amount()
                .to_u64_checked()
                .expect("remainder bucket amount out of range");
            assert_eq!(
                payee_moved, payee_amount,
                "escrow paid a different amount than ruled"
            );
            assert_eq!(
                remainder_moved,
                principal
                    .checked_sub(payee_amount)
                    .expect("remainder underflow"),
                "escrow remainder does not match the principal"
            );
            // Jurors first: take the fees out of the remainder bucket
            // into the fee vault, then pay the parties. Conservation is
            // exact: payee plus payer plus fees equals the principal.
            let fee_bucket = remainder_bucket.take(Amount::from_u64(fees));
            self.fee_vault.deposit(fee_bucket);
            let payer_amount = remainder_moved
                .checked_sub(fees)
                .expect("payer amount underflow");
            assert_eq!(
                payee_amount
                    .checked_add(payer_amount)
                    .expect("settlement total overflow")
                    .checked_add(fees)
                    .expect("settlement total overflow"),
                principal,
                "settlement does not conserve micro-Tari"
            );
            if payee_moved > 0 {
                ComponentManager::get(payee_account).invoke("deposit", args![payee_bucket]);
            }
            if payer_amount > 0 {
                ComponentManager::get(payer_account).invoke("deposit", args![remainder_bucket]);
            }
            // Slashing, per round. Offenders forfeit one juror fee of
            // bond each; the forfeited bonds top up the majority jurors
            // of the same round. Jurors who voted to refuse arbitration
            // are neither paid nor slashed. Jurors who failed to reveal
            // in the replaced round are slashed once, even if they were
            // drawn again and voted with the new majority.
            let appealed = self.dispute(dispute_id).appealed;
            let first_offenders = Self::offenders(&panel, &reveals, first_outcome);
            let mut first_slash_keys = first_offenders;
            for key in failed_jurors {
                if !first_slash_keys.contains(&key) {
                    first_slash_keys.push(key);
                }
            }
            let first_slash = self.slash_keys(&first_slash_keys);
            let leftover_appeal_fee = if appealed {
                let appeal_offenders = Self::offenders(&appeal_panel, &appeal_reveals, outcome);
                let appeal_slash = self.slash_keys(&appeal_offenders);
                let appeal_majority_keys =
                    Self::majority_keys(&appeal_panel, &appeal_reveals, outcome);
                let appeal_fee_total = (appeal_majority_keys.len() as u64)
                    .checked_mul(JUROR_FEE)
                    .expect("appeal fee total overflow");
                let leftover = APPEAL_FEE
                    .checked_sub(appeal_fee_total)
                    .expect("appeal fee leftover underflow");
                self.record_claims(
                    dispute_id,
                    &appeal_majority_keys,
                    appeal_fee_total,
                    appeal_slash,
                );
                leftover
            } else {
                0
            };
            self.record_claims(dispute_id, &first_majority_keys, fees, first_slash);
            if leftover_appeal_fee > 0 {
                let appellant_account = self
                    .dispute(dispute_id)
                    .appellant_account
                    .expect("appeal has no appellant account");
                let refund: Bucket = self
                    .fee_vault
                    .withdraw(Amount::from_u64(leftover_appeal_fee));
                ComponentManager::get(appellant_account).invoke("deposit", args![refund]);
            }
            let dispute = self.dispute_mut(dispute_id);
            dispute.outcome = outcome;
            dispute.payee_amount = payee_amount;
            dispute.fees_paid = fees;
            dispute.settled = true;
        }

        /// Open an appeal: one appeal per dispute, funded by the
        /// appellant, with a fresh future-epoch draw of seven jurors.
        /// Only a party to the dispute can appeal, only after the first
        /// round has reached a majority ruling, and only before the
        /// dispute settles. The fee is exactly APPEAL_FEE in TARI.
        pub fn open_appeal(
            &mut self,
            dispute_id: u64,
            draw_delay: u64,
            fee: Bucket,
            appellant_account: ComponentAddress,
        ) {
            let signer = CallerContext::transaction_signer_public_key();
            let escrow = self.dispute(dispute_id).escrow;
            let payer: RistrettoPublicKeyBytes =
                ComponentManager::get(escrow).call("payer", args![]);
            let payee: RistrettoPublicKeyBytes =
                ComponentManager::get(escrow).call("payee", args![]);
            assert!(
                signer == payer || signer == payee,
                "only a party to the dispute can appeal"
            );
            assert!(
                fee.resource_address() == TARI_TOKEN,
                "appeal fee must be TARI"
            );
            let fee_amount = fee
                .amount()
                .to_u64_checked()
                .expect("appeal fee amount out of range");
            assert_eq!(
                fee_amount, APPEAL_FEE,
                "appeal fee must be exactly APPEAL_FEE"
            );
            {
                let dispute = self.dispute(dispute_id);
                assert!(dispute.drawn, "jury not drawn for this dispute");
                assert!(!dispute.settled, "dispute already settled");
                assert!(!dispute.closed_no_ruling, "dispute closed with no ruling");
                assert!(!dispute.appealed, "appeal already opened");
                assert!(
                    Self::majority_outcome(&dispute.reveals, dispute.panel.len() as u64).is_some(),
                    "first round has no majority ruling to appeal"
                );
            }
            let now = Consensus::current_epoch();
            self.fee_vault.deposit(fee);
            let dispute = self.dispute_mut(dispute_id);
            dispute.appealed = true;
            dispute.appellant = Some(signer);
            dispute.appellant_account = Some(appellant_account);
            dispute.appeal_fee = APPEAL_FEE;
            dispute.appeal_draw_epoch = now
                .checked_add(draw_delay)
                .expect("appeal draw delay overflows epoch range");
            dispute.appeal_draw_hash = Hash32::from([0u8; 32]);
            dispute.appeal_drawn = false;
            dispute.appeal_round = 0;
        }

        /// Execute the appeal jury draw. Only during the locked appeal
        /// draw epoch, only once. Same derivation as the first draw,
        /// with a seven-juror panel and a fresh round nonce.
        pub fn draw_appeal_jury(&mut self, dispute_id: u64) -> Hash32 {
            {
                let dispute = self.dispute(dispute_id);
                assert!(dispute.appealed, "no appeal opened for this dispute");
                assert!(!dispute.appeal_drawn, "appeal jury already drawn");
                assert!(!dispute.settled, "dispute already settled");
                assert!(!dispute.closed_no_ruling, "dispute closed with no ruling");
                assert_eq!(
                    Consensus::current_epoch(),
                    dispute.appeal_draw_epoch,
                    "draw_appeal_jury only executes during the locked draw epoch"
                );
            }
            let hash = Consensus::current_epoch_hash();
            let panel = {
                let dispute = self.dispute(dispute_id);
                Self::derive_panel(
                    &dispute.eligible_pool,
                    hash,
                    dispute_id,
                    dispute.appeal_round,
                    APPEAL_PANEL_SIZE,
                )
            };
            let dispute = self.dispute_mut(dispute_id);
            dispute.appeal_draw_hash = hash;
            dispute.appeal_drawn = true;
            dispute.appeal_panel = panel;
            hash
        }

        /// Reschedule a missed appeal draw to the next epoch, recorded
        /// on chain like the first-round reschedule.
        pub fn reschedule_appeal_draw(&mut self, dispute_id: u64) {
            let dispute = self.dispute_mut(dispute_id);
            assert!(dispute.appealed, "no appeal opened for this dispute");
            assert!(!dispute.appeal_drawn, "appeal jury already drawn");
            assert!(!dispute.settled, "dispute already settled");
            assert!(!dispute.closed_no_ruling, "dispute closed with no ruling");
            assert!(
                Consensus::current_epoch() > dispute.appeal_draw_epoch,
                "appeal draw epoch has not passed yet"
            );
            dispute.appeal_draw_epoch = dispute
                .appeal_draw_epoch
                .checked_add(1)
                .expect("appeal draw epoch overflow");
            dispute.appeal_reschedules = dispute
                .appeal_reschedules
                .checked_add(1)
                .expect("appeal reschedule count overflow");
        }

        /// Commit an appeal vote: post the hash of (vote, salt, dispute
        /// id, juror). Only a drawn appeal juror can commit, once.
        pub fn commit_appeal_vote(&mut self, dispute_id: u64, commitment: Hash32) {
            let signer = CallerContext::transaction_signer_public_key();
            let dispute = self.dispute_mut(dispute_id);
            assert!(dispute.appealed, "no appeal opened for this dispute");
            assert!(dispute.appeal_drawn, "appeal jury not drawn");
            assert!(
                dispute.appeal_panel.contains(&signer),
                "juror not drawn for this appeal"
            );
            assert!(
                !dispute.appeal_commits.iter().any(|c| c.juror == signer),
                "juror already committed an appeal vote"
            );
            dispute.appeal_commits.push(VoteCommit {
                juror: signer,
                commitment,
            });
        }

        /// Reveal an appeal vote: post the vote and salt. The court
        /// recomputes the commitment and it must match. Only once.
        pub fn reveal_appeal_vote(&mut self, dispute_id: u64, vote: u64, salt: Hash32) {
            let signer = CallerContext::transaction_signer_public_key();
            assert!(vote <= VOTE_REFUSE, "invalid vote");
            let expected = Self::vote_commitment(vote, salt, dispute_id, signer);
            let dispute = self.dispute_mut(dispute_id);
            assert!(dispute.appealed, "no appeal opened for this dispute");
            let stored = dispute
                .appeal_commits
                .iter()
                .find(|c| c.juror == signer)
                .map(|c| c.commitment)
                .expect("no appeal vote commitment from this juror");
            assert!(stored == expected, "reveal does not match the commitment");
            assert!(
                !dispute.appeal_reveals.iter().any(|r| r.juror == signer),
                "juror already revealed an appeal vote"
            );
            dispute.appeal_reveals.push(VoteReveal {
                juror: signer,
                vote,
            });
        }

        /// Record a failed appeal round and commit a new future epoch
        /// for the appeal replacement draw. Same rule as the first
        /// round: once per appeal, only when the drawn appeal round
        /// cannot produce a ruling, with the jurors who did not reveal
        /// recorded as failed.
        pub fn replace_appeal_jury(&mut self, dispute_id: u64, draw_delay: u64) {
            {
                let dispute = self.dispute(dispute_id);
                assert!(dispute.appealed, "no appeal opened for this dispute");
                assert!(dispute.appeal_drawn, "appeal jury not drawn");
                assert!(!dispute.settled, "dispute already settled");
                assert!(!dispute.closed_no_ruling, "dispute closed with no ruling");
                assert!(
                    !dispute.appeal_replacement_used,
                    "appeal replacement already used"
                );
                assert!(
                    dispute.appeal_reveals.len() < dispute.appeal_panel.len(),
                    "appeal round is fully revealed"
                );
                assert!(
                    Self::majority_outcome(
                        &dispute.appeal_reveals,
                        dispute.appeal_panel.len() as u64
                    )
                    .is_none(),
                    "appeal round already has a majority ruling"
                );
            }
            let now = Consensus::current_epoch();
            let dispute = self.dispute_mut(dispute_id);
            for juror in dispute.appeal_panel.clone() {
                if !dispute.appeal_reveals.iter().any(|r| r.juror == juror)
                    && !dispute.failed_jurors.contains(&juror)
                {
                    dispute.failed_jurors.push(juror);
                }
            }
            dispute.appeal_draw_epoch = now
                .checked_add(draw_delay)
                .expect("appeal draw delay overflows epoch range");
            dispute.appeal_draw_hash = Hash32::from([0u8; 32]);
            dispute.appeal_drawn = false;
            dispute.appeal_round = dispute
                .appeal_round
                .checked_add(1)
                .expect("appeal draw round overflow");
            dispute.appeal_panel = Vec::new();
            dispute.appeal_commits = Vec::new();
            dispute.appeal_reveals = Vec::new();
            dispute.appeal_replacement_used = true;
        }

        /// Close a dispute with no ruling, the bounded exit of the
        /// ladder. Allowed only when no ruling can be reached: either
        /// the replacement draw has been used and the new round still
        /// has no majority, or a round is fully revealed with no
        /// majority, including an appeal round. The ruling executes with
        /// a zero payee amount, which unfreezes the escrow, and the
        /// whole principal returns to the payer: the opener's fee is
        /// refunded in full, no juror fee is taken. Jurors who were
        /// drawn and did not reveal are slashed, and the forfeited
        /// bonds go to the jurors who did reveal. An unspent appeal fee
        /// returns to the appellant in full.
        pub fn close_no_ruling(
            &mut self,
            dispute_id: u64,
            payee_account: ComponentAddress,
            payer_account: ComponentAddress,
        ) {
            let escrow = {
                let dispute = self.dispute(dispute_id);
                assert!(!dispute.settled, "dispute already settled");
                assert!(
                    !dispute.closed_no_ruling,
                    "dispute already closed with no ruling"
                );
                assert!(dispute.drawn, "jury not drawn for this dispute");
                let round_done = dispute.reveals.len() == dispute.panel.len();
                let appeal_done = dispute.appealed
                    && dispute.appeal_drawn
                    && dispute.appeal_reveals.len() == dispute.appeal_panel.len();
                let first_no_majority =
                    Self::majority_outcome(&dispute.reveals, dispute.panel.len() as u64).is_none();
                let appeal_no_majority = Self::majority_outcome(
                    &dispute.appeal_reveals,
                    dispute.appeal_panel.len() as u64,
                )
                .is_none();
                if dispute.appealed {
                    // The first round already ruled, which is why the
                    // appeal exists. Only the appeal round can still
                    // reach a ruling, so only it gates the close.
                    assert!(
                        appeal_no_majority && (dispute.appeal_replacement_used || appeal_done),
                        "appeal round can still reach a ruling"
                    );
                } else {
                    assert!(
                        first_no_majority && (dispute.replacement_used || round_done),
                        "first round can still reach a ruling"
                    );
                }
                dispute.escrow
            };
            let principal: u64 = ComponentManager::get(escrow).call("vault_balance", args![]);
            // A zero ruling unfreezes the escrow; the remainder is the
            // whole principal, and it all returns to the payer.
            let payee_bucket: Bucket =
                ComponentManager::get(escrow).call("execute_ruling", args![dispute_id, 0u64]);
            let remainder_bucket: Bucket =
                ComponentManager::get(escrow).call("withdraw_remainder", args![]);
            let payee_moved = payee_bucket
                .amount()
                .to_u64_checked()
                .expect("payee bucket amount out of range");
            let remainder_moved = remainder_bucket
                .amount()
                .to_u64_checked()
                .expect("remainder bucket amount out of range");
            assert_eq!(payee_moved, 0, "no-ruling payee amount must be zero");
            assert_eq!(
                remainder_moved, principal,
                "no-ruling remainder must be the whole principal"
            );
            if remainder_moved > 0 {
                ComponentManager::get(payer_account).invoke("deposit", args![remainder_bucket]);
            }
            let _ = payee_account;
            // Slash the jurors who did not reveal, and pay the forfeited
            // bonds to the jurors who did. When nobody revealed, nobody
            // is slashed: there is no one to pay, and the court keeps
            // nothing.
            let (ruling_panel, ruling_reveals, failed_jurors) = {
                let dispute = self.dispute(dispute_id);
                if dispute.appealed {
                    (
                        dispute.appeal_panel.clone(),
                        dispute
                            .appeal_reveals
                            .iter()
                            .map(|r| r.juror)
                            .collect::<Vec<RistrettoPublicKeyBytes>>(),
                        dispute.failed_jurors.clone(),
                    )
                } else {
                    (
                        dispute.panel.clone(),
                        dispute
                            .reveals
                            .iter()
                            .map(|r| r.juror)
                            .collect::<Vec<RistrettoPublicKeyBytes>>(),
                        dispute.failed_jurors.clone(),
                    )
                }
            };
            let slash_total = if ruling_reveals.is_empty() {
                0
            } else {
                let mut slash_keys_list: Vec<RistrettoPublicKeyBytes> = ruling_panel
                    .iter()
                    .filter(|key| !ruling_reveals.contains(key))
                    .cloned()
                    .collect();
                for key in failed_jurors {
                    if !slash_keys_list.contains(&key) {
                        slash_keys_list.push(key);
                    }
                }
                self.slash_keys(&slash_keys_list)
            };
            if !ruling_reveals.is_empty() && slash_total > 0 {
                let dispute = self.dispute_mut(dispute_id);
                for (i, share) in Self::distribute(slash_total, ruling_reveals.len() as u64)
                    .into_iter()
                    .enumerate()
                {
                    if share > 0 {
                        Self::add_claim(dispute, ruling_reveals[i], share);
                    }
                }
            }
            if self.dispute(dispute_id).appealed && self.dispute(dispute_id).appeal_fee > 0 {
                let appellant_account = self
                    .dispute(dispute_id)
                    .appellant_account
                    .expect("appeal has no appellant account");
                let appeal_fee = self.dispute(dispute_id).appeal_fee;
                let refund: Bucket = self.fee_vault.withdraw(Amount::from_u64(appeal_fee));
                ComponentManager::get(appellant_account).invoke("deposit", args![refund]);
            }
            let dispute = self.dispute_mut(dispute_id);
            dispute.closed_no_ruling = true;
            dispute.outcome = NO_OUTCOME;
            dispute.payee_amount = 0;
            dispute.fees_paid = 0;
        }

        /// Claim a fee owed for a dispute: one juror fee plus any
        /// slashed bond share, once. Only the juror the claim belongs
        /// to can claim it.
        pub fn claim_fee(&mut self, dispute_id: u64, account: ComponentAddress) {
            let signer = CallerContext::transaction_signer_public_key();
            let amount = {
                let dispute = self.dispute_mut(dispute_id);
                let claim = dispute
                    .claims
                    .iter_mut()
                    .find(|c| c.juror == signer)
                    .expect("no fee claim for this juror");
                assert!(!claim.claimed, "fee already claimed");
                assert!(claim.amount > 0, "no fee owed for this dispute");
                let amount = claim.amount;
                claim.claimed = true;
                amount
            };
            let bucket = self.fee_vault.withdraw(Amount::from_u64(amount));
            ComponentManager::get(account).invoke("deposit", args![bucket]);
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

        /// True once the dispute has closed with no ruling.
        pub fn dispute_closed_no_ruling(&self, dispute_id: u64) -> bool {
            self.dispute(dispute_id).closed_no_ruling
        }

        /// Ruling outcome once settled: VOTE_PAY, VOTE_REFUND, or
        /// VOTE_SPLIT. NO_OUTCOME (4) until the dispute settles, and
        /// after it closes with no ruling.
        pub fn dispute_outcome(&self, dispute_id: u64) -> u64 {
            self.dispute(dispute_id).outcome
        }

        /// Amount the ruling paid to the payee, in micro-Tari.
        pub fn dispute_payee_amount(&self, dispute_id: u64) -> u64 {
            self.dispute(dispute_id).payee_amount
        }

        /// Juror fees taken from the principal at settle, in micro-Tari.
        pub fn dispute_fees_paid(&self, dispute_id: u64) -> u64 {
            self.dispute(dispute_id).fees_paid
        }

        /// True once the one replacement draw has been used.
        pub fn dispute_replacement_used(&self, dispute_id: u64) -> bool {
            self.dispute(dispute_id).replacement_used
        }

        /// Times the draw was rescheduled after its locked epoch passed.
        pub fn dispute_reschedules(&self, dispute_id: u64) -> u64 {
            self.dispute(dispute_id).reschedules
        }

        /// True once an appeal has been opened on this dispute.
        pub fn dispute_appealed(&self, dispute_id: u64) -> bool {
            self.dispute(dispute_id).appealed
        }

        /// True once the appeal draw has executed.
        pub fn dispute_appeal_drawn(&self, dispute_id: u64) -> bool {
            self.dispute(dispute_id).appeal_drawn
        }

        /// True once the one appeal replacement draw has been used.
        pub fn dispute_appeal_replacement_used(&self, dispute_id: u64) -> bool {
            self.dispute(dispute_id).appeal_replacement_used
        }

        /// Locked appeal draw epoch.
        pub fn dispute_appeal_draw_epoch(&self, dispute_id: u64) -> u64 {
            self.dispute(dispute_id).appeal_draw_epoch
        }

        /// Length of the appeal panel.
        pub fn dispute_appeal_panel_len(&self, dispute_id: u64) -> u64 {
            self.dispute(dispute_id).appeal_panel.len() as u64
        }

        /// One juror key from the appeal panel, by index.
        pub fn dispute_appeal_panel_member(
            &self,
            dispute_id: u64,
            index: u64,
        ) -> RistrettoPublicKeyBytes {
            let dispute = self.dispute(dispute_id);
            assert!(
                index < dispute.appeal_panel.len() as u64,
                "appeal panel index out of range"
            );
            dispute.appeal_panel[index as usize]
        }

        /// Revealed appeal votes so far.
        pub fn dispute_appeal_reveal_count(&self, dispute_id: u64) -> u64 {
            self.dispute(dispute_id).appeal_reveals.len() as u64
        }

        /// Fee owed to one juror for a dispute, in micro-Tari. Zero
        /// when the juror is owed nothing.
        pub fn dispute_claim(&self, dispute_id: u64, juror: RistrettoPublicKeyBytes) -> u64 {
            self.dispute(dispute_id)
                .claims
                .iter()
                .find(|c| c.juror == juror)
                .map(|c| c.amount)
                .unwrap_or(0)
        }

        /// True once the juror has claimed the fee for a dispute.
        pub fn dispute_claimed(&self, dispute_id: u64, juror: RistrettoPublicKeyBytes) -> bool {
            self.dispute(dispute_id)
                .claims
                .iter()
                .find(|c| c.juror == juror)
                .map(|c| c.claimed)
                .unwrap_or(false)
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

        /// The majority outcome of a round, if one exists: one of pay,
        /// refund, or split holding a strict majority of the revealed
        /// votes, with at least a majority of the panel revealed.
        /// Refuse votes never produce an outcome.
        fn majority_outcome(reveals: &[VoteReveal], panel_len: u64) -> Option<u64> {
            let quorum = panel_len / 2 + 1;
            if (reveals.len() as u64) < quorum {
                return None;
            }
            for outcome in [VOTE_PAY, VOTE_REFUND, VOTE_SPLIT] {
                let count = Self::majority_count(reveals, outcome);
                if count * 2 > reveals.len() as u64 {
                    return Some(outcome);
                }
            }
            None
        }

        /// How many revealed votes are for the given outcome.
        fn majority_count(reveals: &[VoteReveal], outcome: u64) -> u64 {
            reveals.iter().filter(|r| r.vote == outcome).count() as u64
        }

        /// The panel members who revealed the given outcome, in panel
        /// order. These are the jurors the ruling pays.
        fn majority_keys(
            panel: &[RistrettoPublicKeyBytes],
            reveals: &[VoteReveal],
            outcome: u64,
        ) -> Vec<RistrettoPublicKeyBytes> {
            panel
                .iter()
                .filter(|key| {
                    reveals
                        .iter()
                        .any(|r| &r.juror == *key && r.vote == outcome)
                })
                .cloned()
                .collect()
        }

        /// The panel members who forfeit bond for a round: everyone
        /// drawn who did not reveal the ruling outcome. That covers
        /// jurors who voted against the majority and jurors who did not
        /// reveal at all. Jurors who voted to refuse arbitration are
        /// not offenders.
        fn offenders(
            panel: &[RistrettoPublicKeyBytes],
            reveals: &[VoteReveal],
            outcome: u64,
        ) -> Vec<RistrettoPublicKeyBytes> {
            panel
                .iter()
                .filter(|key| {
                    let vote = reveals.iter().find(|r| &r.juror == *key).map(|r| r.vote);
                    vote != Some(outcome) && vote != Some(VOTE_REFUSE)
                })
                .cloned()
                .collect()
        }

        /// Split `total` micro-Tari into `count` shares by largest
        /// remainder: every share gets the floor, and the first
        /// `total % count` shares in order get one extra micro-Tari.
        /// The shares always sum to exactly `total`.
        fn distribute(total: u64, count: u64) -> Vec<u64> {
            if count == 0 {
                return Vec::new();
            }
            let base = total / count;
            let extra = total % count;
            (0..count)
                .map(|i| if i < extra { base + 1 } else { base })
                .collect()
        }

        /// Forfeit one juror fee of bond from each listed juror, capped
        /// by the bond the juror still holds, and move the forfeited
        /// total from the bond vault into the fee vault. Returns the
        /// total actually forfeited.
        fn slash_keys(&mut self, keys: &[RistrettoPublicKeyBytes]) -> u64 {
            let mut total: u64 = 0;
            for key in keys {
                if let Some(juror) = self.jurors.iter_mut().find(|j| j.key == *key) {
                    let slashed = SLASH.min(juror.bond);
                    juror.bond = juror
                        .bond
                        .checked_sub(slashed)
                        .expect("juror bond underflow");
                    total = total.checked_add(slashed).expect("slash total overflow");
                }
            }
            if total > 0 {
                let slash_bucket = self.bond_vault.withdraw(Amount::from_u64(total));
                self.fee_vault.deposit(slash_bucket);
            }
            total
        }

        /// Record fee claims for the majority jurors of one round: each
        /// juror is owed a share of the round's fee total plus a share
        /// of the round's forfeited bonds, both split by largest
        /// remainder. A juror who already has a claim, from the other
        /// round, has the new amount added to it.
        fn record_claims(
            &mut self,
            dispute_id: u64,
            majority_keys: &[RistrettoPublicKeyBytes],
            fee_total: u64,
            slash_total: u64,
        ) {
            let fee_shares = Self::distribute(fee_total, majority_keys.len() as u64);
            let slash_shares = Self::distribute(slash_total, majority_keys.len() as u64);
            let dispute = self.dispute_mut(dispute_id);
            for (i, key) in majority_keys.iter().enumerate() {
                let amount = fee_shares[i]
                    .checked_add(slash_shares[i])
                    .expect("fee claim overflow");
                if amount > 0 {
                    Self::add_claim(dispute, *key, amount);
                }
            }
        }

        /// Add a fee claim for one juror, merging with an existing
        /// unclaimed claim for the same juror when there is one.
        fn add_claim(dispute: &mut Dispute, juror: RistrettoPublicKeyBytes, amount: u64) {
            if let Some(claim) = dispute.claims.iter_mut().find(|c| c.juror == juror) {
                claim.amount = claim
                    .amount
                    .checked_add(amount)
                    .expect("fee claim overflow");
            } else {
                dispute.claims.push(FeeClaim {
                    juror,
                    amount,
                    claimed: false,
                });
            }
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
        /// most `target_size` jurors, fewer when the pool is smaller.
        fn derive_panel(
            pool: &[RistrettoPublicKeyBytes],
            draw_hash: Hash32,
            dispute_id: u64,
            round: u64,
            target_size: u64,
        ) -> Vec<RistrettoPublicKeyBytes> {
            let pool_len = pool.len() as u64;
            if pool_len == 0 {
                return Vec::new();
            }
            let target = target_size.min(pool_len) as usize;
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
