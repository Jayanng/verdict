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
    /// Minimum draw delay, in epochs. Every draw commits to a future
    /// epoch at least one epoch ahead, so the draw never uses the
    /// current epoch hash, which is already known when the dispute
    /// opens.
    const MIN_DRAW_DELAY: u64 = 1;
    /// Reveal window, in epochs after the draw epoch. A drawn juror
    /// has this long to reveal. A round can only be replaced once the
    /// window has passed, so a replacement never lands while honest
    /// jurors are still within their time to reveal.
    const REVEAL_WINDOW: u64 = 2;
    /// Commit window, in epochs after the draw epoch. A drawn juror
    /// must commit within this window: a commitment posted after it
    /// is refused, so no juror can wait to see other jurors' revealed
    /// votes before committing. The window is shorter than the reveal
    /// window, so a commitment always precedes its reveal.
    const COMMIT_WINDOW: u64 = 1;
    /// Draw domain folded into the panel derivation. The first round
    /// draws with domain 0 and the appeal with domain 1. The domain
    /// is hashed into the derivation along with the draw hash, the
    /// round, the dispute id, and the slot, so each round's panel is
    /// derived independently. The derivation makes no promise that two
    /// draws produce different panels: panels drawn from different
    /// domains are independent draws, and may still match by chance.
    const DRAW_DOMAIN_FIRST: u64 = 0;
    const DRAW_DOMAIN_APPEAL: u64 = 1;
    /// Times a missed draw window can be rescheduled, per draw. A draw
    /// that keeps being missed stops moving after this many reschedules,
    /// so the bounded exit in the docs holds.
    const MAX_RESCHEDULES: u64 = 3;
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
        /// bond share. A claim of 0 means no fee is owed.
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
        /// Last epoch of the reveal window, set at draw time to the
        /// draw epoch plus REVEAL_WINDOW. A round can only be replaced
        /// once the current epoch has reached this epoch.
        pub reveal_deadline: u64,
        /// Last epoch of the commit window, set at draw time to the
        /// draw epoch plus COMMIT_WINDOW. A commitment posted after
        /// this epoch is refused.
        pub commit_deadline: u64,
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
        /// Drawn jurors who failed to reveal in a replaced first round.
        /// They are slashed when the dispute settles or closes, and the
        /// forfeited bonds top up the first-round majority only.
        pub failed_jurors: Vec<RistrettoPublicKeyBytes>,
        /// Drawn jurors who failed to reveal in a replaced appeal round.
        /// They are slashed when the dispute settles or closes, and the
        /// forfeited bonds top up the appeal majority only, never the
        /// first-round majority.
        pub appeal_failed_jurors: Vec<RistrettoPublicKeyBytes>,
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
        /// Last epoch of the appeal reveal window, set at appeal draw
        /// time to the appeal draw epoch plus REVEAL_WINDOW.
        pub appeal_reveal_deadline: u64,
        /// Last epoch of the appeal commit window, set at appeal draw
        /// time to the appeal draw epoch plus COMMIT_WINDOW.
        pub appeal_commit_deadline: u64,
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
        /// Returns the dispute id. The delay must be at least one epoch,
        /// so the draw never uses the current epoch hash. The eligible
        /// pool must already hold a full panel, so the escrow never
        /// freezes into a dispute that cannot draw. K is a caller-chosen
        /// delay, not a constant, so small windows can be exercised in
        /// tests.
        pub fn open_dispute(&mut self, escrow: ComponentAddress, draw_delay: u64) -> u64 {
            assert!(
                draw_delay >= MIN_DRAW_DELAY,
                "draw delay must be at least one epoch"
            );
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
            // Checked after the freeze, so the escrow's own empty-vault
            // rejection still runs first on an unfunded escrow.
            assert!(
                eligible_pool.len() as u64 >= PANEL_SIZE,
                "not enough eligible jurors to draw a panel"
            );
            self.disputes.push(Dispute {
                escrow,
                open_epoch,
                open_epoch_hash,
                draw_epoch,
                draw_hash: Hash32::from([0u8; 32]),
                reveal_deadline: 0,
                commit_deadline: 0,
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
                appeal_failed_jurors: Vec::new(),
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
                appeal_reveal_deadline: 0,
                appeal_commit_deadline: 0,
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
        /// Evidence can be posted until the dispute settles or closes:
        /// the protocol has no earlier evidence cutoff, but once the
        /// dispute is terminal its record is immutable, so posting to
        /// a settled or closed dispute fails.
        pub fn submit_evidence(
            &mut self,
            dispute_id: u64,
            index: u64,
            sha256: Hash32,
            uri: String,
            bytes_len: u64,
        ) {
            assert!(index < EVIDENCE_LIMIT, "evidence index out of range");
            let dispute = self.dispute(dispute_id);
            assert!(!dispute.settled, "dispute already settled");
            assert!(!dispute.closed_no_ruling, "dispute closed with no ruling");
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
                assert!(
                    dispute.eligible_pool.len() as u64 >= PANEL_SIZE,
                    "not enough eligible jurors to draw a panel"
                );
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
                    DRAW_DOMAIN_FIRST,
                )
            };
            let deadline = Consensus::current_epoch()
                .checked_add(REVEAL_WINDOW)
                .expect("reveal deadline overflows epoch range");
            let commit_deadline = Consensus::current_epoch()
                .checked_add(COMMIT_WINDOW)
                .expect("commit deadline overflows epoch range");
            let dispute = self.dispute_mut(dispute_id);
            dispute.draw_hash = hash;
            dispute.drawn = true;
            dispute.reveal_deadline = deadline;
            dispute.commit_deadline = commit_deadline;
            dispute.panel = panel;
            hash
        }

        /// Reschedule a missed draw: if the locked draw epoch has passed
        /// without a draw, move the commitment past the current epoch.
        /// The new draw epoch is the current epoch plus one, never the
        /// current epoch itself, so the rescheduled draw still uses a
        /// future epoch hash. The reschedule is recorded on chain, so
        /// it cannot happen quietly. A draw can be rescheduled at most
        /// three times: after the third reschedule the draw must execute
        /// in its locked epoch or the dispute is left to the fallback.
        pub fn reschedule_draw(&mut self, dispute_id: u64) {
            let dispute = self.dispute_mut(dispute_id);
            assert!(!dispute.drawn, "jury already drawn for this dispute");
            assert!(!dispute.settled, "dispute already settled");
            assert!(!dispute.closed_no_ruling, "dispute closed with no ruling");
            assert!(
                dispute.reschedules < MAX_RESCHEDULES,
                "draw reschedule limit reached"
            );
            assert!(
                Consensus::current_epoch() > dispute.draw_epoch,
                "draw epoch has not passed yet"
            );
            dispute.draw_epoch = Consensus::current_epoch()
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
        /// revealed, and no outcome holds a majority of the reveals. Only
        /// a party to the dispute can call it, and only once the reveal
        /// window has passed, so no replacement can land while honest
        /// jurors are still within their time to reveal. The delay must
        /// be at least one epoch, so the replacement draw still uses a
        /// future epoch hash. The jurors who did not reveal are recorded
        /// as failed; they are slashed when the dispute settles or
        /// closes. The panel, commits, and reveals are cleared, the round
        /// nonce advances, and the next draw executes only in the new
        /// locked epoch.
        pub fn replace_jury(&mut self, dispute_id: u64, draw_delay: u64) {
            assert!(
                draw_delay >= MIN_DRAW_DELAY,
                "draw delay must be at least one epoch"
            );
            let signer = CallerContext::transaction_signer_public_key();
            let escrow = self.dispute(dispute_id).escrow;
            let payer: RistrettoPublicKeyBytes =
                ComponentManager::get(escrow).call("payer", args![]);
            let payee: RistrettoPublicKeyBytes =
                ComponentManager::get(escrow).call("payee", args![]);
            assert!(
                signer == payer || signer == payee,
                "only a party to the dispute can replace the jury"
            );
            {
                let dispute = self.dispute(dispute_id);
                assert!(dispute.drawn, "jury not drawn for this dispute");
                assert!(!dispute.settled, "dispute already settled");
                assert!(!dispute.closed_no_ruling, "dispute closed with no ruling");
                assert!(!dispute.appealed, "dispute is under appeal");
                assert!(!dispute.replacement_used, "replacement already used");
                assert!(
                    Consensus::current_epoch() >= dispute.reveal_deadline,
                    "reveal window has not passed yet"
                );
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
        /// juror). Only a drawn juror can commit, and only once. A
        /// commitment is refused once the commit window has passed, so
        /// no juror can wait to see other jurors' revealed votes before
        /// committing, and no vote can be added to a settled or closed
        /// dispute.
        pub fn commit_vote(&mut self, dispute_id: u64, commitment: Hash32) {
            let signer = CallerContext::transaction_signer_public_key();
            let dispute = self.dispute_mut(dispute_id);
            assert!(dispute.drawn, "jury not drawn for this dispute");
            assert!(!dispute.settled, "dispute already settled");
            assert!(!dispute.closed_no_ruling, "dispute closed with no ruling");
            assert!(
                Consensus::current_epoch() <= dispute.commit_deadline,
                "commit window has passed"
            );
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
        /// the commitment and it must match. Only once per juror. A
        /// reveal is refused once the reveal window has passed, and no
        /// vote can be added to a settled or closed dispute.
        pub fn reveal_vote(&mut self, dispute_id: u64, vote: u64, salt: Hash32) {
            let signer = CallerContext::transaction_signer_public_key();
            assert!(vote <= VOTE_REFUSE, "invalid vote");
            let expected = Self::vote_commitment(vote, salt, dispute_id, signer);
            let dispute = self.dispute_mut(dispute_id);
            assert!(dispute.drawn, "jury not drawn for this dispute");
            assert!(!dispute.settled, "dispute already settled");
            assert!(!dispute.closed_no_ruling, "dispute closed with no ruling");
            assert!(
                Consensus::current_epoch() <= dispute.reveal_deadline,
                "reveal window has passed"
            );
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

        /// Settle a dispute: the ruling is the majority of the full
        /// panel, never a caller-supplied amount. The court reads the
        /// escrow vault balance as the principal, works out the payee
        /// amount from the ruling outcome, orders the ruling, and routes
        /// both buckets the escrow returns. The payout accounts are
        /// never taken from the caller: the court reads the payer and
        /// the payee keys from the escrow and derives their accounts
        /// from those keys, so no caller can redirect the proceeds to
        /// an account of their own. Jurors are paid first: the
        /// fees owed to the majority jurors are taken from the remainder
        /// bucket into the fee vault before the payer is paid. Costs are
        /// capped at the principal. Exactly once. Settle runs only once
        /// the reveal window has passed, for the first round and, when
        /// the dispute is under appeal, for the appeal round too, so a
        /// juror who still has time to reveal is never counted among
        /// the offenders and slashed. When the dispute is
        /// under appeal, the appeal ruling settles it, the appeal jurors
        /// are paid from the appeal fee, and the unspent appeal fee
        /// returns to the appellant. The court never holds the money
        /// beyond the fee vault.
        pub fn settle(&mut self, dispute_id: u64) {
            // Read the ruling and the principal before any state
            // changes, so a failed settle leaves the dispute untouched.
            let (escrow, outcome, first_outcome, principal, payer_key, payee_key) = {
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
                // The reveal window must have passed before anyone can
                // be slashed for not revealing: a juror inside their
                // window is not an offender.
                assert!(
                    Consensus::current_epoch() >= dispute.reveal_deadline,
                    "reveal window has not passed yet"
                );
                if dispute.appealed {
                    assert!(
                        Consensus::current_epoch() >= dispute.appeal_reveal_deadline,
                        "appeal reveal window has not passed yet"
                    );
                }
                let escrow = dispute.escrow;
                let principal: u64 = ComponentManager::get(escrow).call("vault_balance", args![]);
                let payer_key: RistrettoPublicKeyBytes =
                    ComponentManager::get(escrow).call("payer", args![]);
                let payee_key: RistrettoPublicKeyBytes =
                    ComponentManager::get(escrow).call("payee", args![]);
                (
                    escrow,
                    outcome,
                    first_outcome,
                    principal,
                    payer_key,
                    payee_key,
                )
            };
            // The payout accounts come from the escrow, never from the
            // caller, so the proceeds can only land with the parties to
            // the deal.
            let payee_account = Self::account_address(payee_key);
            let payer_account = Self::account_address(payer_key);
            // Fees owed from the principal: one juror fee per juror in
            // the first-round majority, capped at the principal. When
            // the dispute is under appeal, the appeal jurors are paid
            // from the appeal fee instead, so the principal funds only
            // the first-round majority.
            let (panel, reveals, appeal_panel, appeal_reveals, failed_jurors, appeal_failed_jurors) = {
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
                    dispute.appeal_failed_jurors.clone(),
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
                        .expect("split division failed")
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
                let mut appeal_slash_keys = appeal_offenders;
                for key in appeal_failed_jurors {
                    if !appeal_slash_keys.contains(&key) {
                        appeal_slash_keys.push(key);
                    }
                }
                let appeal_slash = self.slash_keys(&appeal_slash_keys);
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
        /// dispute settles. The fee is exactly APPEAL_FEE in TARI. The
        /// delay must be at least one epoch, and the eligible pool must
        /// already hold a full appeal panel of seven jurors. The
        /// account the unspent appeal fee returns to is derived from
        /// the appellant's signing key, never taken from the caller, so
        /// no appellant can name a refund destination of their own.
        pub fn open_appeal(&mut self, dispute_id: u64, draw_delay: u64, fee: Bucket) {
            assert!(
                draw_delay >= MIN_DRAW_DELAY,
                "draw delay must be at least one epoch"
            );
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
                assert!(
                    dispute.eligible_pool.len() as u64 >= APPEAL_PANEL_SIZE,
                    "not enough eligible jurors to draw an appeal panel"
                );
            }
            let now = Consensus::current_epoch();
            self.fee_vault.deposit(fee);
            let appellant_account = Self::account_address(signer);
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
        /// with a seven-juror panel and a fresh round nonce, in the
        /// appeal draw domain, so the appeal panel is derived
        /// independently of any first-round panel drawn from the same
        /// hash.
        pub fn draw_appeal_jury(&mut self, dispute_id: u64) -> Hash32 {
            {
                let dispute = self.dispute(dispute_id);
                assert!(dispute.appealed, "no appeal opened for this dispute");
                assert!(!dispute.appeal_drawn, "appeal jury already drawn");
                assert!(!dispute.settled, "dispute already settled");
                assert!(!dispute.closed_no_ruling, "dispute closed with no ruling");
                assert!(
                    dispute.eligible_pool.len() as u64 >= APPEAL_PANEL_SIZE,
                    "not enough eligible jurors to draw an appeal panel"
                );
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
                    DRAW_DOMAIN_APPEAL,
                )
            };
            let deadline = Consensus::current_epoch()
                .checked_add(REVEAL_WINDOW)
                .expect("appeal reveal deadline overflows epoch range");
            let commit_deadline = Consensus::current_epoch()
                .checked_add(COMMIT_WINDOW)
                .expect("appeal commit deadline overflows epoch range");
            let dispute = self.dispute_mut(dispute_id);
            dispute.appeal_draw_hash = hash;
            dispute.appeal_drawn = true;
            dispute.appeal_reveal_deadline = deadline;
            dispute.appeal_commit_deadline = commit_deadline;
            dispute.appeal_panel = panel;
            hash
        }

        /// Reschedule a missed appeal draw past the current epoch,
        /// recorded on chain like the first-round reschedule. The new
        /// appeal draw epoch is the current epoch plus one, never the
        /// current epoch itself. An appeal draw can be rescheduled at
        /// most three times, the same cap as the first-round draw.
        pub fn reschedule_appeal_draw(&mut self, dispute_id: u64) {
            let dispute = self.dispute_mut(dispute_id);
            assert!(dispute.appealed, "no appeal opened for this dispute");
            assert!(!dispute.appeal_drawn, "appeal jury already drawn");
            assert!(!dispute.settled, "dispute already settled");
            assert!(!dispute.closed_no_ruling, "dispute closed with no ruling");
            assert!(
                dispute.appeal_reschedules < MAX_RESCHEDULES,
                "appeal draw reschedule limit reached"
            );
            assert!(
                Consensus::current_epoch() > dispute.appeal_draw_epoch,
                "appeal draw epoch has not passed yet"
            );
            dispute.appeal_draw_epoch = Consensus::current_epoch()
                .checked_add(1)
                .expect("appeal draw epoch overflow");
            dispute.appeal_reschedules = dispute
                .appeal_reschedules
                .checked_add(1)
                .expect("appeal reschedule count overflow");
        }

        /// Commit an appeal vote: post the hash of (vote, salt, dispute
        /// id, juror). Only a drawn appeal juror can commit, once. A
        /// commitment is refused once the appeal commit window has
        /// passed, and no vote can be added to a settled or closed
        /// dispute.
        pub fn commit_appeal_vote(&mut self, dispute_id: u64, commitment: Hash32) {
            let signer = CallerContext::transaction_signer_public_key();
            let dispute = self.dispute_mut(dispute_id);
            assert!(dispute.appealed, "no appeal opened for this dispute");
            assert!(dispute.appeal_drawn, "appeal jury not drawn");
            assert!(!dispute.settled, "dispute already settled");
            assert!(!dispute.closed_no_ruling, "dispute closed with no ruling");
            assert!(
                Consensus::current_epoch() <= dispute.appeal_commit_deadline,
                "appeal commit window has passed"
            );
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
        /// recomputes the commitment and it must match. Only once. A
        /// reveal is refused once the appeal reveal window has passed,
        /// and no vote can be added to a settled or closed dispute.
        pub fn reveal_appeal_vote(&mut self, dispute_id: u64, vote: u64, salt: Hash32) {
            let signer = CallerContext::transaction_signer_public_key();
            assert!(vote <= VOTE_REFUSE, "invalid vote");
            let expected = Self::vote_commitment(vote, salt, dispute_id, signer);
            let dispute = self.dispute_mut(dispute_id);
            assert!(dispute.appealed, "no appeal opened for this dispute");
            assert!(dispute.appeal_drawn, "appeal jury not drawn");
            assert!(!dispute.settled, "dispute already settled");
            assert!(!dispute.closed_no_ruling, "dispute closed with no ruling");
            assert!(
                Consensus::current_epoch() <= dispute.appeal_reveal_deadline,
                "appeal reveal window has passed"
            );
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
        /// recorded as failed. Only a party to the dispute can call it,
        /// and only once the appeal reveal window has passed.
        pub fn replace_appeal_jury(&mut self, dispute_id: u64, draw_delay: u64) {
            assert!(
                draw_delay >= MIN_DRAW_DELAY,
                "draw delay must be at least one epoch"
            );
            let signer = CallerContext::transaction_signer_public_key();
            let escrow = self.dispute(dispute_id).escrow;
            let payer: RistrettoPublicKeyBytes =
                ComponentManager::get(escrow).call("payer", args![]);
            let payee: RistrettoPublicKeyBytes =
                ComponentManager::get(escrow).call("payee", args![]);
            assert!(
                signer == payer || signer == payee,
                "only a party to the dispute can replace the appeal jury"
            );
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
                    Consensus::current_epoch() >= dispute.appeal_reveal_deadline,
                    "appeal reveal window has not passed yet"
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
                    && !dispute.appeal_failed_jurors.contains(&juror)
                {
                    dispute.appeal_failed_jurors.push(juror);
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
        /// ladder. Allowed only when no ruling can be reached: a round
        /// is finished when it is fully revealed, and a replacement
        /// round is also finished once its own reveal window has
        /// passed with no majority, so a replacement round always gets
        /// its full window to vote before the close can run, and a
        /// round that stalls after its replacement still exits. A draw
        /// that never executed closes the same way once its reschedules
        /// are spent and its final locked epoch has passed, so a missed
        /// draw can never leave the escrow frozen: the principal
        /// returns to the payer in full, no juror fee is taken, and
        /// any appeal fee returns to the appellant in full. Under
        /// appeal, both rounds must be done before the dispute exits:
        /// the first round is final once its reveal window has passed,
        /// since no more first-round reveals can land after the
        /// deadline, and the appeal round must be finished the same
        /// way. The ruling executes with a payee amount of 0, which
        /// unfreezes the escrow, and the whole principal returns to
        /// the payer: the opener's fee is refunded in full, no juror
        /// fee is taken. The payer account is read from the escrow,
        /// never taken from the caller, so no caller can redirect the
        /// principal. Jurors who were drawn and did not reveal are
        /// slashed, and the forfeited bonds go to the jurors who did
        /// reveal, other than jurors who voted to refuse: a refusing
        /// juror is neither paid nor slashed. When nobody who revealed
        /// is payable, nobody is slashed: there is no one to pay, and
        /// the court keeps nothing. An unspent appeal fee returns to
        /// the appellant in full.
        pub fn close_no_ruling(&mut self, dispute_id: u64) {
            let escrow = {
                let dispute = self.dispute(dispute_id);
                assert!(!dispute.settled, "dispute already settled");
                assert!(
                    !dispute.closed_no_ruling,
                    "dispute already closed with no ruling"
                );
                let now = Consensus::current_epoch();
                // A round is finished when it is fully revealed. A
                // replacement round is also finished once its own
                // reveal window has passed with no majority: the new
                // jurors have had their full window, and the dispute
                // still exits instead of staying frozen.
                let round_finished = dispute.drawn
                    && (dispute.reveals.len() == dispute.panel.len()
                        || (dispute.replacement_used && now >= dispute.reveal_deadline));
                let appeal_finished = dispute.appeal_drawn
                    && (dispute.appeal_reveals.len() == dispute.appeal_panel.len()
                        || (dispute.appeal_replacement_used
                            && now >= dispute.appeal_reveal_deadline));
                // A first round that already ruled is final once its
                // reveal window has passed: no more first-round reveals
                // can land after the deadline, so its majority cannot
                // change. A close under appeal needs that as well as
                // the appeal round finished, so both rounds are done
                // before the dispute exits.
                let first_round_final = dispute.drawn
                    && (dispute.reveals.len() == dispute.panel.len()
                        || now >= dispute.reveal_deadline);
                // A draw that never executed can never execute once
                // its reschedules are spent and its final locked epoch
                // has passed. The dispute then closes the same way
                // instead of staying frozen forever.
                let never_drawn = !dispute.drawn
                    && dispute.reschedules >= MAX_RESCHEDULES
                    && now > dispute.draw_epoch;
                let appeal_never_drawn = dispute.appealed
                    && !dispute.appeal_drawn
                    && dispute.appeal_reschedules >= MAX_RESCHEDULES
                    && now > dispute.appeal_draw_epoch;
                let first_no_majority =
                    Self::majority_outcome(&dispute.reveals, dispute.panel.len() as u64).is_none();
                let appeal_no_majority = Self::majority_outcome(
                    &dispute.appeal_reveals,
                    dispute.appeal_panel.len() as u64,
                )
                .is_none();
                if dispute.appealed {
                    assert!(first_round_final, "first round can still reach a ruling");
                    assert!(
                        appeal_no_majority && (appeal_finished || appeal_never_drawn),
                        "appeal round can still reach a ruling"
                    );
                } else {
                    assert!(
                        (first_no_majority && round_finished) || never_drawn,
                        "first round can still reach a ruling"
                    );
                }
                dispute.escrow
            };
            let principal: u64 = ComponentManager::get(escrow).call("vault_balance", args![]);
            // A ruling with a payee amount of 0 unfreezes the escrow;
            // the remainder is the whole principal, and it all returns
            // to the payer.
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
            assert_eq!(payee_moved, 0, "no-ruling payee amount must be 0");
            assert_eq!(
                remainder_moved, principal,
                "no-ruling remainder must be the whole principal"
            );
            // The whole principal returns to the payer named by the
            // escrow, never to an account the caller supplies.
            let payer_key: RistrettoPublicKeyBytes =
                ComponentManager::get(escrow).call("payer", args![]);
            let payer_account = Self::account_address(payer_key);
            if remainder_moved > 0 {
                ComponentManager::get(payer_account).invoke("deposit", args![remainder_bucket]);
            }
            // Slash the jurors who did not reveal, and pay the forfeited
            // bonds to the jurors who did reveal, other than jurors who
            // voted to refuse: a refusing juror is neither paid nor
            // slashed. When nobody who revealed is payable, nobody is
            // slashed: there is no one to pay, and the court keeps
            // nothing. Failed jurors are kept per round, so an appeal
            // close slashes appeal failures only.
            let (ruling_panel, revealed_keys, payable_keys, failed_jurors) = {
                let dispute = self.dispute(dispute_id);
                if dispute.appealed {
                    (
                        dispute.appeal_panel.clone(),
                        dispute
                            .appeal_reveals
                            .iter()
                            .map(|r| r.juror)
                            .collect::<Vec<RistrettoPublicKeyBytes>>(),
                        dispute
                            .appeal_reveals
                            .iter()
                            .filter(|r| r.vote != VOTE_REFUSE)
                            .map(|r| r.juror)
                            .collect::<Vec<RistrettoPublicKeyBytes>>(),
                        dispute.appeal_failed_jurors.clone(),
                    )
                } else {
                    (
                        dispute.panel.clone(),
                        dispute
                            .reveals
                            .iter()
                            .map(|r| r.juror)
                            .collect::<Vec<RistrettoPublicKeyBytes>>(),
                        dispute
                            .reveals
                            .iter()
                            .filter(|r| r.vote != VOTE_REFUSE)
                            .map(|r| r.juror)
                            .collect::<Vec<RistrettoPublicKeyBytes>>(),
                        dispute.failed_jurors.clone(),
                    )
                }
            };
            let slash_total = if payable_keys.is_empty() {
                0
            } else {
                let mut slash_keys_list: Vec<RistrettoPublicKeyBytes> = ruling_panel
                    .iter()
                    .filter(|key| !revealed_keys.contains(key))
                    .cloned()
                    .collect();
                for key in failed_jurors {
                    if !slash_keys_list.contains(&key) {
                        slash_keys_list.push(key);
                    }
                }
                self.slash_keys(&slash_keys_list)
            };
            if !payable_keys.is_empty() && slash_total > 0 {
                let dispute = self.dispute_mut(dispute_id);
                for (i, share) in Self::distribute(slash_total, payable_keys.len() as u64)
                    .into_iter()
                    .enumerate()
                {
                    if share > 0 {
                        Self::add_claim(dispute, payable_keys[i], share);
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

        /// Times the appeal draw was rescheduled after its locked epoch passed.
        pub fn dispute_appeal_reschedules(&self, dispute_id: u64) -> u64 {
            self.dispute(dispute_id).appeal_reschedules
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

        /// Fee owed to one juror for a dispute, in micro-Tari. 0
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

        /// Last epoch of the reveal window for a dispute, set at draw
        /// time. A round can only be replaced once the current epoch
        /// has reached this epoch.
        pub fn dispute_reveal_deadline(&self, dispute_id: u64) -> u64 {
            self.dispute(dispute_id).reveal_deadline
        }

        /// Last epoch of the commit window for a dispute, set at draw
        /// time. A commitment posted after this epoch is refused.
        pub fn dispute_commit_deadline(&self, dispute_id: u64) -> u64 {
            self.dispute(dispute_id).commit_deadline
        }

        /// Last epoch of the appeal reveal window for a dispute.
        pub fn dispute_appeal_reveal_deadline(&self, dispute_id: u64) -> u64 {
            self.dispute(dispute_id).appeal_reveal_deadline
        }

        /// Last epoch of the appeal commit window for a dispute.
        pub fn dispute_appeal_commit_deadline(&self, dispute_id: u64) -> u64 {
            self.dispute(dispute_id).appeal_commit_deadline
        }

        /// Appeal draw round nonce, 0 for the first appeal draw.
        pub fn dispute_appeal_round(&self, dispute_id: u64) -> u64 {
            self.dispute(dispute_id).appeal_round
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
        /// refund, or split holding a strict majority of the full panel,
        /// with at least a majority of the panel revealed first.
        /// Refuse votes never produce an outcome.
        fn majority_outcome(reveals: &[VoteReveal], panel_len: u64) -> Option<u64> {
            let quorum = panel_len / 2 + 1;
            if (reveals.len() as u64) < quorum {
                return None;
            }
            for outcome in [VOTE_PAY, VOTE_REFUND, VOTE_SPLIT] {
                let count = Self::majority_count(reveals, outcome);
                if count * 2 > panel_len {
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
        /// round, the draw domain, and the frozen eligible pool. For
        /// each slot, hash the draw hash together with the domain, the
        /// round, the dispute id, and the slot, and take the first eight
        /// bytes of the digest modulo the pool size, probing forward
        /// until an unpicked juror is found. The domain is a separate
        /// hash input, so each round's panel is derived independently
        /// of the other round's: no arithmetic between the domain and
        /// the round can make two draws collide. Returns at most
        /// `target_size` jurors, fewer when the pool is smaller.
        fn derive_panel(
            pool: &[RistrettoPublicKeyBytes],
            draw_hash: Hash32,
            dispute_id: u64,
            round: u64,
            target_size: u64,
            domain: u64,
        ) -> Vec<RistrettoPublicKeyBytes> {
            let pool_len = pool.len() as u64;
            if pool_len == 0 {
                return Vec::new();
            }
            let target = target_size.min(pool_len) as usize;
            let mut panel: Vec<RistrettoPublicKeyBytes> = Vec::new();
            for slot in 0..target as u64 {
                let digest = intrinsics::hash_sha256_parts(&[
                    draw_hash.as_ref(),
                    &domain.to_le_bytes(),
                    &round.to_le_bytes(),
                    &dispute_id.to_le_bytes(),
                    &slot.to_le_bytes(),
                ]);
                let bytes: &[u8] = digest.as_ref();
                let mut window = [0u8; 8];
                window.copy_from_slice(&bytes[0..8]);
                let value = u64::from_le_bytes(window);
                let mut candidate = value % pool_len;
                while panel.contains(&pool[candidate as usize]) {
                    candidate = (candidate + 1) % pool_len;
                }
                panel.push(pool[candidate as usize]);
            }
            panel
        }

        /// The account address of a public key, derived the same way
        /// the engine derives an account created for that key: the
        /// Blake2b-256 hash, under the engine component-address domain,
        /// of the account template address (all bytes) and the key.
        /// The engine hashes the template address and the key in their
        /// Borsh form, so the key carries its 4-byte little-endian
        /// length prefix. Settle and the no-ruling close derive the
        /// payer and payee accounts from the escrow keys with this
        /// function, so the payout accounts are never caller-supplied.
        fn account_address(key: RistrettoPublicKeyBytes) -> ComponentAddress {
            // Domain tag: the 8-byte little-endian length 41, then
            // "com.tari.ootle.engine", ".v", "0", ".", "ComponentAddress".
            const DOMAIN_TAG: &[u8] =
                b"\x29\x00\x00\x00\x00\x00\x00\x00com.tari.ootle.engine.v0.ComponentAddress";
            let hash = intrinsics::hash_blake2b_parts(&[
                DOMAIN_TAG,
                &[0u8; 32],
                &32u32.to_le_bytes(),
                key.as_ref(),
            ]);
            ComponentAddress::new(tari_template_lib::types::ObjectKey::from_array(
                hash.leading_bytes::<32>(),
            ))
        }

        /// The account address derived from a public key, published so
        /// an external verifier can check where a settlement pays.
        pub fn account_address_for_key(&self, key: RistrettoPublicKeyBytes) -> ComponentAddress {
            Self::account_address(key)
        }
    }
}
