// Verdict simulation engine, mirrored rule for rule from
// templates/verdict/src/lib.rs at 278344b.
//
// This is a simulation, not a live court. It runs the full dispute
// lifecycle in the browser with no wallet, no funds, and no chain: create,
// open, evidence, draw, commit, reveal, settle, appeal, replacement, and
// the no-ruling fallback. Every rule and every rejection message matches
// the template, so the parity suite in tests/parity.test.js can prove the
// simulation and the template agree. Nothing here moves real micro-Tari
// and nothing here is a chain call.

import {
  PANEL_SIZE,
  APPEAL_PANEL_SIZE,
  JUROR_FEE,
  BOND_FLOOR,
  APPEAL_FEE,
  SLASH,
  EVIDENCE_LIMIT,
  MIN_DRAW_DELAY,
  REVEAL_WINDOW,
  DRAW_DOMAIN_FIRST,
  DRAW_DOMAIN_APPEAL,
  VOTE_PAY,
  VOTE_REFUND,
  VOTE_SPLIT,
  VOTE_REFUSE,
  derivePanel,
  majorityOutcome,
  settleAmounts,
  splitLargestRemainder,
  sha256Hex,
  voteCommitmentBytes,
} from "./verdict.js";

export const NO_OUTCOME = 4;

function fail(message) {
  throw new Error(message);
}

function defaultEpochHash(epoch) {
  return sha256Hex(`verdict-sim-epoch-${epoch}`);
}

// One simulated escrow: a vault funded by the payer, frozen by a dispute,
// executed at most once by the court.
export class SimEscrow {
  constructor(id, payer, payee, principal) {
    this.id = id;
    this.payer = payer;
    this.payee = payee;
    this.principal = principal;
    this.vault = principal;
    this.frozen = false;
    this.disputeId = null;
    this.rulingExecuted = false;
    this.remainderWithdrawn = false;
  }

  freezeForDispute(disputeId) {
    if (this.vault <= 0) fail("escrow vault is empty");
    if (this.frozen) fail("escrow already frozen for a dispute");
    this.frozen = true;
    this.disputeId = disputeId;
  }

  executeRuling(disputeId, payeeAmount) {
    if (!this.frozen || this.disputeId !== disputeId) fail("escrow not frozen for this dispute");
    if (this.rulingExecuted) fail("ruling already executed");
    if (payeeAmount > this.vault) fail("payee amount exceeds the principal");
    this.rulingExecuted = true;
    this.vault -= payeeAmount;
    return payeeAmount;
  }

  withdrawRemainder() {
    if (!this.rulingExecuted) fail("no ruling has executed");
    if (this.remainderWithdrawn) fail("remainder already withdrawn");
    this.remainderWithdrawn = true;
    const remainder = this.vault;
    this.vault = 0;
    return remainder;
  }
}

// The simulated court. Accounts are plain string ids with balances in
// micro-Tari; juror keys are the same strings the sample data uses.
export class SimCourt {
  constructor(poolKeys = []) {
    this.epoch = 0;
    this.epochHashes = new Map();
    this.balances = new Map();
    this.jurors = new Map();
    this.bonds = new Map();
    this.escrows = new Map();
    this.disputes = [];
    this.feeVault = 0;
    this.poolKeys = [...poolKeys];
  }

  fund(account, amount) {
    this.balances.set(account, (this.balances.get(account) || 0) + amount);
  }

  balance(account) {
    return this.balances.get(account) || 0;
  }

  setEpoch(epoch) {
    this.epoch = epoch;
  }

  setEpochHash(epoch, hashHex) {
    this.epochHashes.set(epoch, hashHex);
  }

  epochHash(epoch) {
    return this.epochHashes.get(epoch) || defaultEpochHash(epoch);
  }

  registerJuror(key, bond) {
    if (this.jurors.has(key)) fail("juror already registered");
    if (bond < BOND_FLOOR) fail("bond below the floor");
    if (this.balance(key) < bond) fail("insufficient balance to post the bond");
    this.balances.set(key, this.balance(key) - bond);
    this.jurors.set(key, { key, bond });
    this.bonds.set(key, bond);
    return key;
  }

  jurorCount() {
    return this.jurors.size;
  }

  createEscrow(id, payer, payee, principal) {
    if (this.balance(payer) < principal) fail("insufficient balance to fund the escrow");
    this.balances.set(payer, this.balance(payer) - principal);
    const escrow = new SimEscrow(id, payer, payee, principal);
    this.escrows.set(id, escrow);
    return escrow;
  }

  dispute(disputeId) {
    const dispute = this.disputes[disputeId];
    if (!dispute) fail("dispute id out of range");
    return dispute;
  }

  openDispute(escrowId, drawDelay) {
    if (drawDelay < MIN_DRAW_DELAY) fail("draw delay must be at least one epoch");
    const escrow = this.escrows.get(escrowId);
    if (!escrow) fail("escrow not found");
    const disputeId = this.disputes.length;
    const openEpoch = this.epoch;
    const openEpochHash = this.epochHash(openEpoch);
    const drawEpoch = openEpoch + drawDelay;
    const eligiblePool = [...this.jurors.keys()];
    escrow.freezeForDispute(disputeId);
    if (eligiblePool.length < PANEL_SIZE) fail("not enough eligible jurors to draw a panel");
    this.disputes.push({
      id: disputeId,
      escrowId,
      payer: escrow.payer,
      payee: escrow.payee,
      principal: escrow.principal,
      openEpoch,
      openEpochHash,
      drawEpoch,
      drawHash: null,
      revealDeadline: 0,
      drawn: false,
      settled: false,
      closedNoRuling: false,
      outcome: NO_OUTCOME,
      payeeAmount: 0,
      payerAmount: 0,
      feesPaid: 0,
      round: 0,
      replacementUsed: false,
      reschedules: 0,
      failedJurors: [],
      claims: new Map(),
      eligiblePool,
      panel: [],
      payerEvidence: [null, null, null],
      payeeEvidence: [null, null, null],
      commits: new Map(),
      reveals: new Map(),
      appealed: false,
      appealFee: 0,
      appellantAccount: null,
      appealDrawEpoch: 0,
      appealDrawHash: null,
      appealRevealDeadline: 0,
      appealDrawn: false,
      appealRound: 0,
      appealReplacementUsed: false,
      appealReschedules: 0,
      appealFailedJurors: [],
      appealPanel: [],
      appealCommits: new Map(),
      appealReveals: new Map(),
    });
    return disputeId;
  }

  submitEvidence(disputeId, caller, side, index, hash, uri) {
    const dispute = this.dispute(disputeId);
    if (dispute.settled) fail("dispute already settled");
    if (dispute.closedNoRuling) fail("dispute closed with no ruling");
    if (side !== 0 && side !== 1) fail("invalid evidence side");
    const party = side === 0 ? dispute.payer : dispute.payee;
    if (caller !== party) fail("only a party to the dispute can submit evidence");
    if (index >= EVIDENCE_LIMIT) fail("evidence index out of range");
    const slot = side === 0 ? dispute.payerEvidence : dispute.payeeEvidence;
    if (slot[index] !== null) fail("evidence already posted at this index");
    slot[index] = { side, index, sha256: hash, uri };
    return slot[index];
  }

  drawJury(disputeId) {
    const dispute = this.dispute(disputeId);
    if (dispute.drawn) fail("jury already drawn for this dispute");
    if (dispute.settled) fail("dispute already settled");
    if (dispute.closedNoRuling) fail("dispute closed with no ruling");
    if (dispute.eligiblePool.length < PANEL_SIZE) fail("not enough eligible jurors to draw a panel");
    if (this.epoch !== dispute.drawEpoch) fail("draw_jury only executes during the locked draw epoch");
    const drawHash = this.epochHash(dispute.drawEpoch);
    const indexes = derivePanel(
      dispute.eligiblePool,
      drawHash,
      disputeId,
      dispute.round,
      DRAW_DOMAIN_FIRST,
      PANEL_SIZE,
    );
    dispute.drawHash = drawHash;
    dispute.panel = indexes.map((i) => dispute.eligiblePool[i]);
    dispute.revealDeadline = dispute.drawEpoch + REVEAL_WINDOW;
    dispute.drawn = true;
    return { drawHash, panel: [...dispute.panel] };
  }

  rescheduleDraw(disputeId) {
    const dispute = this.dispute(disputeId);
    if (dispute.drawn) fail("jury already drawn for this dispute");
    if (dispute.settled) fail("dispute already settled");
    if (dispute.closedNoRuling) fail("dispute closed with no ruling");
    if (this.epoch <= dispute.drawEpoch) fail("draw epoch has not passed yet");
    dispute.drawEpoch = this.epoch + 1;
    dispute.reschedules += 1;
    return dispute.drawEpoch;
  }

  commitVote(disputeId, juror, vote, salt) {
    const dispute = this.dispute(disputeId);
    if (!dispute.drawn) fail("jury not drawn for this dispute");
    if (!dispute.panel.includes(juror)) fail("juror not drawn for this dispute");
    if (dispute.commits.has(juror)) fail("juror already committed a vote");
    if (vote > VOTE_REFUSE) fail("invalid vote");
    const commitment = sha256Hex(voteCommitmentBytes(vote, salt, disputeId, juror));
    dispute.commits.set(juror, commitment);
    return commitment;
  }

  revealVote(disputeId, juror, vote, salt) {
    const dispute = this.dispute(disputeId);
    const stored = dispute.commits.get(juror);
    if (stored === undefined) fail("no vote commitment from this juror");
    if (dispute.reveals.has(juror)) fail("juror already revealed a vote");
    const expected = sha256Hex(voteCommitmentBytes(vote, salt, disputeId, juror));
    if (stored !== expected) fail("reveal does not match the commitment");
    dispute.reveals.set(juror, vote);
    return vote;
  }

  replaceJury(disputeId, caller, drawDelay) {
    const dispute = this.dispute(disputeId);
    if (drawDelay < MIN_DRAW_DELAY) fail("draw delay must be at least one epoch");
    if (caller !== dispute.payer && caller !== dispute.payee) {
      fail("only a party to the dispute can replace the jury");
    }
    if (!dispute.drawn) fail("jury not drawn for this dispute");
    if (dispute.replacementUsed) fail("replacement already used");
    if (this.epoch < dispute.revealDeadline) fail("reveal window has not passed yet");
    if (dispute.settled) fail("dispute already settled");
    if (dispute.closedNoRuling) fail("dispute closed with no ruling");
    dispute.failedJurors = dispute.panel.filter((key) => !dispute.reveals.has(key));
    dispute.panel = [];
    dispute.commits = new Map();
    dispute.reveals = new Map();
    dispute.round += 1;
    dispute.drawEpoch = this.epoch + drawDelay;
    dispute.drawn = false;
    dispute.replacementUsed = true;
    return dispute.drawEpoch;
  }

  firstOutcome(dispute) {
    const reveals = [...dispute.reveals.values()];
    return majorityOutcome(reveals, dispute.panel.length);
  }

  appealOutcome(dispute) {
    const reveals = [...dispute.appealReveals.values()];
    return majorityOutcome(reveals, dispute.appealPanel.length);
  }

  majorityKeys(panel, reveals, outcome) {
    return panel.filter((key) => reveals.get(key) === outcome);
  }

  offenders(panel, reveals, outcome) {
    return panel.filter(
      (key) =>
        reveals.has(key) && reveals.get(key) !== outcome && reveals.get(key) !== VOTE_REFUSE,
    );
  }

  slashKeys(keys) {
    let total = 0;
    for (const key of keys) {
      const bond = this.bonds.get(key) || 0;
      const slash = Math.min(SLASH, bond);
      this.bonds.set(key, bond - slash);
      total += slash;
    }
    this.feeVault += total;
    return total;
  }

  recordClaims(dispute, keys, feeTotal, slashTotal) {
    const feeEach = keys.length ? Math.floor(feeTotal / keys.length) : 0;
    const slashShares = keys.length
      ? splitLargestRemainder(slashTotal, keys.map(() => 1))
      : [];
    keys.forEach((key, i) => {
      const amount = feeEach + (slashShares[i] || 0);
      if (amount > 0) {
        const existing = dispute.claims.get(key);
        dispute.claims.set(key, {
          amount: (existing?.amount || 0) + amount,
          claimed: existing?.claimed || false,
        });
      }
    });
  }

  settle(disputeId) {
    const dispute = this.dispute(disputeId);
    if (dispute.settled) fail("dispute already settled");
    if (dispute.closedNoRuling) fail("dispute closed with no ruling");
    const first = this.firstOutcome(dispute);
    if (first.vote === null && !dispute.appealed) fail("first round has no majority ruling");
    let outcome;
    let appeal = null;
    if (dispute.appealed) {
      appeal = this.appealOutcome(dispute);
      if (appeal.vote === null) fail("appeal round has no majority ruling");
      outcome = appeal.vote;
    } else {
      outcome = first.vote;
    }
    const escrow = this.escrows.get(dispute.escrowId);
    const amounts = settleAmounts(dispute.principal, outcome, first.count);
    const fees = amounts.fees;
    const payeeMoved = escrow.executeRuling(disputeId, amounts.payeeAmount);
    if (payeeMoved !== amounts.payeeAmount) fail("escrow paid a different amount than ruled");
    const remainder = escrow.withdrawRemainder();
    if (remainder !== dispute.principal - amounts.payeeAmount) {
      fail("escrow remainder does not match the principal");
    }
    this.feeVault += fees;
    const payerMoved = remainder - fees;
    this.fund(dispute.payee, payeeMoved);
    this.fund(dispute.payer, payerMoved);
    if (payeeMoved + payerMoved + fees !== dispute.principal) {
      fail("settlement does not conserve micro-Tari");
    }
    const firstMajority = this.majorityKeys(dispute.panel, dispute.reveals, first.vote);
    const firstOffenders = this.offenders(dispute.panel, dispute.reveals, first.vote);
    const firstSlashKeys = [...firstOffenders];
    for (const key of dispute.failedJurors) {
      if (!firstSlashKeys.includes(key)) firstSlashKeys.push(key);
    }
    const firstSlash = this.slashKeys(firstSlashKeys);
    let leftoverAppealFee = 0;
    if (dispute.appealed) {
      const appealMajority = this.majorityKeys(
        dispute.appealPanel,
        dispute.appealReveals,
        outcome,
      );
      const appealOffenders = this.offenders(dispute.appealPanel, dispute.appealReveals, outcome);
      const appealSlashKeys = [...appealOffenders];
      for (const key of dispute.appealFailedJurors) {
        if (!appealSlashKeys.includes(key)) appealSlashKeys.push(key);
      }
      const appealSlash = this.slashKeys(appealSlashKeys);
      const appealFeeTotal = appealMajority.length * JUROR_FEE;
      leftoverAppealFee = APPEAL_FEE - appealFeeTotal;
      this.recordClaims(dispute, appealMajority, appealFeeTotal, appealSlash);
    }
    this.recordClaims(dispute, firstMajority, fees, firstSlash);
    if (leftoverAppealFee > 0) {
      this.fund(dispute.appellantAccount, leftoverAppealFee);
      this.feeVault -= leftoverAppealFee;
    }
    dispute.outcome = outcome;
    dispute.payeeAmount = amounts.payeeAmount;
    dispute.payerAmount = amounts.payerAmount;
    dispute.feesPaid = fees;
    dispute.settled = true;
    return {
      outcome,
      payeeAmount: amounts.payeeAmount,
      payerAmount: amounts.payerAmount,
      fees,
      leftoverAppealFee,
    };
  }

  openAppeal(disputeId, caller, fee, drawDelay) {
    const dispute = this.dispute(disputeId);
    if (drawDelay < MIN_DRAW_DELAY) fail("draw delay must be at least one epoch");
    if (caller !== dispute.payer && caller !== dispute.payee) {
      fail("only a party to the dispute can appeal");
    }
    if (fee !== APPEAL_FEE) fail("appeal fee must be exactly APPEAL_FEE");
    if (!dispute.drawn) fail("jury not drawn for this dispute");
    if (dispute.settled) fail("dispute already settled");
    if (dispute.closedNoRuling) fail("dispute closed with no ruling");
    if (dispute.appealed) fail("appeal already opened");
    const first = this.firstOutcome(dispute);
    if (first.vote === null) fail("first round has no majority ruling to appeal");
    if (this.jurors.size < APPEAL_PANEL_SIZE) {
      fail("not enough eligible jurors to draw an appeal panel");
    }
    if (this.balance(caller) < fee) fail("insufficient balance to post the appeal fee");
    this.balances.set(caller, this.balance(caller) - fee);
    this.feeVault += fee;
    dispute.appealed = true;
    dispute.appealFee = fee;
    dispute.appellantAccount = caller;
    dispute.appealDrawEpoch = this.epoch + drawDelay;
    return dispute.appealDrawEpoch;
  }

  drawAppealJury(disputeId) {
    const dispute = this.dispute(disputeId);
    if (!dispute.appealed) fail("no appeal opened for this dispute");
    if (dispute.appealDrawn) fail("appeal jury already drawn");
    if (dispute.settled) fail("dispute already settled");
    if (dispute.closedNoRuling) fail("dispute closed with no ruling");
    if (this.jurors.size < APPEAL_PANEL_SIZE) {
      fail("not enough eligible jurors to draw an appeal panel");
    }
    if (this.epoch !== dispute.appealDrawEpoch) {
      fail("draw_appeal_jury only executes during the locked draw epoch");
    }
    const drawHash = this.epochHash(dispute.appealDrawEpoch);
    const pool = [...this.jurors.keys()];
    const indexes = derivePanel(
      pool,
      drawHash,
      disputeId,
      dispute.appealRound,
      DRAW_DOMAIN_APPEAL,
      APPEAL_PANEL_SIZE,
    );
    dispute.appealDrawHash = drawHash;
    dispute.appealPanel = indexes.map((i) => pool[i]);
    dispute.appealRevealDeadline = dispute.appealDrawEpoch + REVEAL_WINDOW;
    dispute.appealDrawn = true;
    return { drawHash, panel: [...dispute.appealPanel] };
  }

  rescheduleAppealDraw(disputeId) {
    const dispute = this.dispute(disputeId);
    if (!dispute.appealed) fail("no appeal opened for this dispute");
    if (dispute.appealDrawn) fail("appeal jury already drawn");
    if (dispute.settled) fail("dispute already settled");
    if (dispute.closedNoRuling) fail("dispute closed with no ruling");
    if (this.epoch <= dispute.appealDrawEpoch) fail("appeal draw epoch has not passed yet");
    dispute.appealDrawEpoch = this.epoch + 1;
    dispute.appealReschedules += 1;
    return dispute.appealDrawEpoch;
  }

  commitAppealVote(disputeId, juror, vote, salt) {
    const dispute = this.dispute(disputeId);
    if (!dispute.appealed) fail("no appeal opened for this dispute");
    if (!dispute.appealDrawn) fail("appeal jury not drawn");
    if (!dispute.appealPanel.includes(juror)) fail("juror not drawn for this appeal");
    if (dispute.appealCommits.has(juror)) fail("juror already committed an appeal vote");
    if (vote > VOTE_REFUSE) fail("invalid vote");
    const commitment = sha256Hex(voteCommitmentBytes(vote, salt, disputeId, juror));
    dispute.appealCommits.set(juror, commitment);
    return commitment;
  }

  revealAppealVote(disputeId, juror, vote, salt) {
    const dispute = this.dispute(disputeId);
    const stored = dispute.appealCommits.get(juror);
    if (stored === undefined) fail("no appeal vote commitment from this juror");
    if (dispute.appealReveals.has(juror)) fail("juror already revealed an appeal vote");
    const expected = sha256Hex(voteCommitmentBytes(vote, salt, disputeId, juror));
    if (stored !== expected) fail("reveal does not match the commitment");
    dispute.appealReveals.set(juror, vote);
    return vote;
  }

  replaceAppealJury(disputeId, caller, drawDelay) {
    const dispute = this.dispute(disputeId);
    if (drawDelay < MIN_DRAW_DELAY) fail("draw delay must be at least one epoch");
    if (caller !== dispute.payer && caller !== dispute.payee) {
      fail("only a party to the dispute can replace the appeal jury");
    }
    if (!dispute.appealed) fail("no appeal opened for this dispute");
    if (!dispute.appealDrawn) fail("appeal jury not drawn");
    if (dispute.appealReplacementUsed) fail("appeal replacement already used");
    if (this.epoch < dispute.appealRevealDeadline) {
      fail("appeal reveal window has not passed yet");
    }
    if (dispute.settled) fail("dispute already settled");
    if (dispute.closedNoRuling) fail("dispute closed with no ruling");
    dispute.appealFailedJurors = dispute.appealPanel.filter(
      (key) => !dispute.appealReveals.has(key),
    );
    dispute.appealPanel = [];
    dispute.appealCommits = new Map();
    dispute.appealReveals = new Map();
    dispute.appealRound += 1;
    dispute.appealDrawEpoch = this.epoch + drawDelay;
    dispute.appealDrawn = false;
    dispute.appealReplacementUsed = true;
    return dispute.appealDrawEpoch;
  }

  closeNoRuling(disputeId) {
    const dispute = this.dispute(disputeId);
    if (dispute.settled) fail("dispute already settled");
    if (dispute.closedNoRuling) fail("dispute already closed with no ruling");
    if (!dispute.drawn) fail("jury not drawn for this dispute");
    const roundDone = dispute.reveals.size === dispute.panel.length;
    const appealDone =
      dispute.appealed &&
      dispute.appealDrawn &&
      dispute.appealReveals.size === dispute.appealPanel.length;
    const firstNoMajority = this.firstOutcome(dispute).vote === null;
    const appealNoMajority = this.appealOutcome(dispute).vote === null;
    if (dispute.appealed) {
      if (!(appealNoMajority && (dispute.appealReplacementUsed || appealDone))) {
        fail("appeal round can still reach a ruling");
      }
    } else if (!(firstNoMajority && (dispute.replacementUsed || roundDone))) {
      fail("first round can still reach a ruling");
    }
    const escrow = this.escrows.get(dispute.escrowId);
    const principal = escrow.vault;
    const payeeMoved = escrow.executeRuling(disputeId, 0);
    if (payeeMoved !== 0) fail("no-ruling payee amount must be 0");
    const remainder = escrow.withdrawRemainder();
    if (remainder !== principal) fail("no-ruling remainder must be the whole principal");
    this.fund(dispute.payer, remainder);
    const rulingPanel = dispute.appealed ? dispute.appealPanel : dispute.panel;
    const rulingReveals = dispute.appealed
      ? [...dispute.appealReveals.keys()]
      : [...dispute.reveals.keys()];
    const failedJurors = dispute.appealed
      ? dispute.appealFailedJurors
      : dispute.failedJurors;
    let slashTotal = 0;
    if (rulingReveals.length > 0) {
      const slashKeys = rulingPanel.filter((key) => !rulingReveals.includes(key));
      for (const key of failedJurors) {
        if (!slashKeys.includes(key)) slashKeys.push(key);
      }
      slashTotal = this.slashKeys(slashKeys);
      if (slashTotal > 0) {
        const shares = splitLargestRemainder(slashTotal, rulingReveals.map(() => 1));
        rulingReveals.forEach((key, i) => {
          if (shares[i] > 0) {
            dispute.claims.set(key, { amount: shares[i], claimed: false });
          }
        });
      }
    }
    if (dispute.appealed && dispute.appealFee > 0) {
      this.fund(dispute.appellantAccount, dispute.appealFee);
      this.feeVault -= dispute.appealFee;
    }
    dispute.closedNoRuling = true;
    dispute.outcome = NO_OUTCOME;
    dispute.payeeAmount = 0;
    dispute.payerAmount = remainder;
    dispute.feesPaid = 0;
    return { payerAmount: remainder, slashTotal };
  }

  claimFee(disputeId, juror) {
    const dispute = this.dispute(disputeId);
    const claim = dispute.claims.get(juror);
    if (!claim) fail("no fee claim for this juror");
    if (claim.claimed) fail("fee already claimed");
    if (claim.amount <= 0) fail("no fee owed for this dispute");
    if (this.feeVault < claim.amount) fail("fee vault cannot cover the claim");
    this.feeVault -= claim.amount;
    claim.claimed = true;
    this.fund(juror, claim.amount);
    return claim.amount;
  }

  disputeClaim(disputeId, juror) {
    const dispute = this.dispute(disputeId);
    return dispute.claims.get(juror)?.amount || 0;
  }

  // A plain snapshot of a dispute for the walkthrough page.
  snapshot(disputeId) {
    const dispute = this.dispute(disputeId);
    const escrow = this.escrows.get(dispute.escrowId);
    return {
      id: dispute.id,
      payer: dispute.payer,
      payee: dispute.payee,
      principal: dispute.principal,
      vault: escrow.vault,
      openEpoch: dispute.openEpoch,
      drawEpoch: dispute.drawEpoch,
      revealDeadline: dispute.revealDeadline,
      drawn: dispute.drawn,
      settled: dispute.settled,
      closedNoRuling: dispute.closedNoRuling,
      outcome: dispute.outcome,
      payeeAmount: dispute.payeeAmount,
      payerAmount: dispute.payerAmount,
      feesPaid: dispute.feesPaid,
      round: dispute.round,
      replacementUsed: dispute.replacementUsed,
      reschedules: dispute.reschedules,
      panel: [...dispute.panel],
      reveals: [...dispute.reveals.entries()].map(([juror, vote]) => ({ juror, vote })),
      appealed: dispute.appealed,
      appealDrawEpoch: dispute.appealDrawEpoch,
      appealRevealDeadline: dispute.appealRevealDeadline,
      appealDrawn: dispute.appealDrawn,
      appealPanel: [...dispute.appealPanel],
      appealReveals: [...dispute.appealReveals.entries()].map(([juror, vote]) => ({
        juror,
        vote,
      })),
      appealReplacementUsed: dispute.appealReplacementUsed,
      appealReschedules: dispute.appealReschedules,
      claims: [...dispute.claims.entries()].map(([juror, claim]) => ({
        juror,
        amount: claim.amount,
        claimed: claim.claimed,
      })),
    };
  }
}

// Vote labels for the walkthrough page, same values as VOTE_LABELS.
export const SIM_VOTE_LABELS = {
  [VOTE_PAY]: "pay",
  [VOTE_REFUND]: "refund",
  [VOTE_SPLIT]: "split",
  [VOTE_REFUSE]: "refuse to arbitrate",
};
