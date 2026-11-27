// Verdict simulation engine, mirrored rule for rule from
// templates/verdict/src/lib.rs at 64cbb43.
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
  MAX_PRINCIPAL_FEE,
  BOND_FLOOR,
  APPEAL_FEE,
  SLASH,
  EVIDENCE_LIMIT,
  MIN_DRAW_DELAY,
  MAX_RESCHEDULES,
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
  hexToBytes,
  sha256Hex,
  voteCommitmentBytes,
} from "./verdict.js";

export const NO_OUTCOME = 4;

// The identity the simulated court uses when it calls the escrow. The
// simulated escrow accepts settlement calls from exactly this identity,
// the same way the real escrow accepts them only from the court named as
// arbitrator at creation.
export const COURT_ID = "verdict-court";

function fail(message) {
  throw new Error(message);
}

// Amounts in the simulation are BigInt, so amounts above the safe integer
// range keep every micro-Tari. A Number amount must be a safe whole number;
// anything else is refused rather than rounded or wrapped.
function amount(value) {
  if (typeof value === "bigint") {
    if (value < 0n) fail("amount must not be negative");
    return value;
  }
  if (typeof value === "number" && Number.isSafeInteger(value) && value >= 0) {
    return BigInt(value);
  }
  throw new Error("amount must be a whole number of micro-Tari");
}

function validVote(vote) {
  if (!Number.isInteger(vote) || vote < 0 || vote > VOTE_REFUSE) {
    fail("invalid vote");
  }
}

// A draw delay must be a safe whole number of epochs, at least the
// minimum: a fractional delay would produce a draw epoch that cannot
// exist, and a delay above the safe integer range would round on
// addition, so distinct epochs could collapse into one.
function validDelay(delay) {
  if (!Number.isSafeInteger(delay)) {
    fail("draw delay must be a whole number of epochs within the safe integer range");
  }
  if (delay < MIN_DRAW_DELAY) fail("draw delay must be at least one epoch");
}

// An epoch sum must stay inside the safe integer range: two safe
// integers can add past it, and the result would round, so distinct
// epochs could collapse into one. Every epoch sum in the simulation
// goes through here.
function safeEpochSum(a, b) {
  const sum = a + b;
  if (!Number.isSafeInteger(sum)) {
    fail("epoch sum is outside the safe integer range");
  }
  return sum;
}

function defaultEpochHash(epoch) {
  return sha256Hex(`verdict-sim-epoch-${epoch}`);
}

// One simulated escrow: a vault funded by the payer, frozen by a dispute,
// executed at most once by the court. Only the court identity may freeze
// an escrow or execute a ruling: any other caller is refused, the same
// way the real escrow refuses every caller but the court named as
// arbitrator at creation.
// executeRuling takes the outcome and the amounts pair (payee amount,
// payer amount), where the payer amount is gross, before the court takes
// juror fees out of it, and returns both amounts in one call, so the
// vault is empty after a ruling and no second call is needed.
export class SimEscrow {
  // The court id must be passed explicitly: there is no default, so
  // omitting it refuses the call instead of silently authenticating
  // as the court. The court identity is the escrow's arbitrator.
  constructor(id, payer, payee, principal, courtId) {
    if (typeof courtId !== "string" || courtId.length === 0) {
      fail("court id must be a non-empty string");
    }
    if (typeof id !== "string" || id.length === 0) {
      fail("escrow id must be a non-empty string");
    }
    if (typeof payer !== "string" || payer.length === 0) {
      fail("payer must be a non-empty string");
    }
    if (typeof payee !== "string" || payee.length === 0) {
      fail("payee must be a non-empty string");
    }
    if (payer === payee) {
      fail("payee must differ from the payer");
    }
    this.id = id;
    this.payer = payer;
    this.payee = payee;
    this.principal = amount(principal);
    this.vault = this.principal;
    this.frozen = false;
    this.disputeId = null;
    this.rulingExecuted = false;
    this.courtId = courtId;
  }

  // The caller must be passed explicitly: there is no default, so
  // omitting it refuses the call instead of authenticating as the
  // court. Only the court identity may freeze an escrow.
  freezeForDispute(disputeId, caller) {
    if (caller !== this.courtId) fail("only the court can freeze an escrow");
    if (this.vault <= 0n) fail("escrow vault is empty");
    if (this.frozen) fail("escrow already frozen for a dispute");
    this.frozen = true;
    this.disputeId = disputeId;
  }

  // The caller must be passed explicitly: there is no default, so
  // omitting it refuses the call instead of authenticating as the
  // court. Only the court identity may execute a ruling.
  executeRuling(disputeId, outcome, amounts, caller) {
    if (caller !== this.courtId) fail("only the court can execute a ruling");
    if (this.rulingExecuted) fail("ruling already executed");
    if (!this.frozen || this.disputeId !== disputeId) fail("escrow not frozen for this dispute");
    if (outcome !== VOTE_PAY && outcome !== VOTE_REFUND && outcome !== VOTE_SPLIT && outcome !== NO_OUTCOME) {
      fail("invalid ruling outcome");
    }
    if (!Array.isArray(amounts) || amounts.length !== 2) {
      fail("ruling amounts must be a pair of amounts");
    }
    for (const value of amounts) {
      const isWhole =
        (typeof value === "bigint" && value >= 0n) ||
        (typeof value === "number" && Number.isSafeInteger(value) && value >= 0);
      if (!isWhole) {
        fail("ruling amounts must be whole numbers of micro-Tari");
      }
    }
    const [payeeAmount, payerAmount] = amounts.map((value) => amount(value));
    if (outcome === VOTE_REFUND && payeeAmount !== 0n) {
      fail("refund pays the payee nothing");
    }
    if (outcome === NO_OUTCOME && payeeAmount !== 0n) {
      fail("no ruling pays the payee nothing");
    }
    if (payeeAmount + payerAmount !== this.vault) {
      fail("ruling amounts must equal the vault balance");
    }
    // The ruling shape checks, after conservation and before any
    // mutation, mirroring the template's escrow exactly: a pay ruling
    // hands the payer only the juror fee, and a split pays both sides
    // in near-equal halves. The rejection strings match the template's.
    const maxFee = BigInt(MAX_PRINCIPAL_FEE);
    if (outcome === VOTE_PAY && payerAmount > maxFee) {
      fail("pay fee exceeds the maximum juror fee total");
    }
    if (outcome === VOTE_SPLIT) {
      if (payeeAmount <= 0n) fail("split must pay the payee");
      if (payerAmount <= 0n) fail("split must pay the payer");
      if (payeeAmount - payerAmount > 1n) {
        fail("split payee exceeds the payer by more than one micro-Tari");
      }
      if (payerAmount - payeeAmount > maxFee) {
        fail("split payer exceeds the payee by more than the maximum juror fee total");
      }
    }
    this.rulingExecuted = true;
    this.frozen = false;
    this.vault = 0n;
    return [payeeAmount, payerAmount];
  }
}

// The simulated court. Accounts are plain string ids with balances in
// micro-Tari; juror keys are the same strings the sample data uses. The
// court calls the escrow with its own identity, COURT_ID, and the escrow
// refuses every other caller.
export class SimCourt {
  constructor(poolKeys = []) {
    this.epoch = 0;
    this.epochHashes = new Map();
    this.balances = new Map();
    this.jurors = new Map();
    this.bonds = new Map();
    this.escrows = new Map();
    this.disputes = [];
    this.feeVault = 0n;
    this.poolKeys = [...poolKeys];
    this.courtId = COURT_ID;
  }

  fund(account, value) {
    this.balances.set(account, this.balance(account) + amount(value));
  }

  balance(account) {
    return this.balances.get(account) || 0n;
  }

  setEpoch(epoch) {
    if (!Number.isSafeInteger(epoch) || epoch < 0) {
      fail("epoch must be a non-negative whole number within the safe integer range");
    }
    this.epoch = epoch;
  }

  setEpochHash(epoch, hashHex) {
    if (!Number.isSafeInteger(epoch) || epoch < 0) {
      fail("epoch must be a non-negative whole number within the safe integer range");
    }
    const hashBytes = hexToBytes(hashHex);
    if (hashBytes.length !== 32) fail("epoch hash must be 32 bytes of hex");
    this.epochHashes.set(epoch, hashHex);
  }

  epochHash(epoch) {
    return this.epochHashes.get(epoch) || defaultEpochHash(epoch);
  }

  registerJuror(key, bond) {
    if (this.jurors.has(key)) fail("juror already registered");
    const bondAmount = amount(bond);
    if (bondAmount < BigInt(BOND_FLOOR)) fail("bond below the floor");
    if (this.balance(key) < bondAmount) fail("insufficient balance to post the bond");
    this.balances.set(key, this.balance(key) - bondAmount);
    this.jurors.set(key, { key, bond: bondAmount });
    this.bonds.set(key, bondAmount);
    return key;
  }

  jurorCount() {
    return this.jurors.size;
  }

  createEscrow(id, payer, payee, principal) {
    if (this.escrows.has(id)) fail("escrow already exists");
    if (typeof payer !== "string" || payer === "") fail("invalid payer");
    if (typeof payee !== "string" || payee === "") fail("invalid payee");
    if (payer === payee) fail("payee must differ from the payer");
    const principalAmount = amount(principal);
    if (principalAmount <= 0n) fail("principal must be more than 0");
    if (this.balance(payer) < principalAmount) {
      fail("insufficient balance to fund the escrow");
    }
    this.balances.set(payer, this.balance(payer) - principalAmount);
    const escrow = new SimEscrow(id, payer, payee, principalAmount, this.courtId);
    this.escrows.set(id, escrow);
    return escrow;
  }

  dispute(disputeId) {
    const dispute = this.disputes[disputeId];
    if (!dispute) fail("dispute id out of range");
    return dispute;
  }

  // A snapshot of everything settlement can touch, so a failing
  // settlement can be rolled back exactly. Disputes are plain data and
  // clone cleanly; escrows keep their class, so their fields are copied
  // back onto the same objects instead.
  snapshotState() {
    return {
      disputes: this.disputes.map((d) => structuredClone(d)),
      escrows: new Map(
        [...this.escrows.entries()].map(([id, escrow]) => [id, { ...escrow }]),
      ),
      balances: new Map(this.balances),
      feeVault: this.feeVault,
      bonds: new Map(this.bonds),
      jurors: new Map(
        [...this.jurors.entries()].map(([key, juror]) => [key, { ...juror }]),
      ),
    };
  }

  restoreState(snapshot) {
    this.disputes = snapshot.disputes;
    for (const [id, fields] of snapshot.escrows) {
      Object.assign(this.escrows.get(id), fields);
    }
    this.balances = snapshot.balances;
    this.feeVault = snapshot.feeVault;
    this.bonds = snapshot.bonds;
    this.jurors = snapshot.jurors;
  }

  openDispute(escrowId, drawDelay) {
    validDelay(drawDelay);
    const escrow = this.escrows.get(escrowId);
    if (!escrow) fail("escrow not found");
    const disputeId = this.disputes.length;
    const openEpoch = this.epoch;
    const openEpochHash = this.epochHash(openEpoch);
    const drawEpoch = safeEpochSum(openEpoch, drawDelay);
    const eligiblePool = [...this.jurors.keys()];
    // All validation runs before the freeze: the simulation has no
    // rollback, so a failed opening must leave the escrow unfrozen.
    // The template freezes first and relies on the transaction rolling
    // back, which the simulation cannot do.
    if (eligiblePool.length < PANEL_SIZE) fail("not enough eligible jurors to draw a panel");
    escrow.freezeForDispute(disputeId, this.courtId);
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
      commitDeadline: 0,
      drawn: false,
      settled: false,
      closedNoRuling: false,
      outcome: NO_OUTCOME,
      payeeAmount: 0n,
      payerAmount: 0n,
      feesPaid: 0n,
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
      appealFee: 0n,
      appellantAccount: null,
      appealDrawEpoch: 0,
      appealDrawHash: null,
      appealRevealDeadline: 0,
      appealCommitDeadline: 0,
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
    if (!Number.isInteger(index) || index < 0 || index >= EVIDENCE_LIMIT) {
      fail("evidence index out of range");
    }
    const slot = side === 0 ? dispute.payerEvidence : dispute.payeeEvidence;
    if (slot[index] !== null) fail("evidence already posted at this index");
    // The template takes the hash as a 32-byte hash type, so the
    // simulation checks the same: a hash that is not 32 bytes of hex
    // is refused with a defined message, not stored.
    let hashBytes;
    try {
      hashBytes = hexToBytes(hash);
    } catch {
      fail("evidence hash must be 32 bytes of hex");
    }
    if (hashBytes.length !== 32) fail("evidence hash must be 32 bytes of hex");
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
    // The deadlines are computed before any state changes, so a
    // refused sum leaves the dispute exactly as it was.
    const revealDeadline = safeEpochSum(dispute.drawEpoch, REVEAL_WINDOW);
    const commitDeadline = safeEpochSum(dispute.drawEpoch, 1);
    dispute.drawHash = drawHash;
    dispute.panel = indexes.map((i) => dispute.eligiblePool[i]);
    dispute.revealDeadline = revealDeadline;
    dispute.commitDeadline = commitDeadline;
    dispute.drawn = true;
    return { drawHash, panel: [...dispute.panel] };
  }

  rescheduleDraw(disputeId) {
    const dispute = this.dispute(disputeId);
    if (dispute.drawn) fail("jury already drawn for this dispute");
    if (dispute.settled) fail("dispute already settled");
    if (dispute.closedNoRuling) fail("dispute closed with no ruling");
    if (this.epoch <= dispute.drawEpoch) fail("draw epoch has not passed yet");
    if (dispute.reschedules >= MAX_RESCHEDULES) fail("draw reschedule limit reached");
    dispute.drawEpoch = safeEpochSum(this.epoch, 1);
    dispute.reschedules += 1;
    return dispute.drawEpoch;
  }

  commitVote(disputeId, juror, vote, salt) {
    const dispute = this.dispute(disputeId);
    if (!dispute.drawn) fail("jury not drawn for this dispute");
    if (dispute.settled) fail("dispute already settled");
    if (dispute.closedNoRuling) fail("dispute closed with no ruling");
    if (this.epoch > dispute.commitDeadline) fail("commit window has passed");
    if (!dispute.panel.includes(juror)) fail("juror not drawn for this dispute");
    if (dispute.commits.has(juror)) fail("juror already committed a vote");
    validVote(vote);
    const commitment = sha256Hex(voteCommitmentBytes(vote, salt, disputeId, juror));
    dispute.commits.set(juror, commitment);
    return commitment;
  }

  revealVote(disputeId, juror, vote, salt) {
    const dispute = this.dispute(disputeId);
    if (!dispute.drawn) fail("jury not drawn for this dispute");
    if (dispute.settled) fail("dispute already settled");
    if (dispute.closedNoRuling) fail("dispute closed with no ruling");
    if (this.epoch <= dispute.commitDeadline) fail("commit window has not ended");
    if (this.epoch > dispute.revealDeadline) fail("reveal window has passed");
    validVote(vote);
    const stored = dispute.commits.get(juror);
    if (stored === undefined) fail("no vote commitment from this juror");
    if (dispute.reveals.has(juror)) fail("juror already revealed a vote");
    const expected = sha256Hex(voteCommitmentBytes(vote, salt, disputeId, juror));
    if (stored !== expected) fail("reveal does not match the commitment");
    dispute.reveals.set(juror, vote);
    return vote;
  }

  // Anyone can call a replacement: an incomplete round must not
  // depend on a party submitting a transaction, or the dispute freezes
  // when neither party acts.
  replaceJury(disputeId, caller, drawDelay) {
    const dispute = this.dispute(disputeId);
    validDelay(drawDelay);
    if (!dispute.drawn) fail("jury not drawn for this dispute");
    if (dispute.replacementUsed) fail("replacement already used");
    if (this.epoch <= dispute.revealDeadline) fail("reveal window has not passed yet");
    if (dispute.settled) fail("dispute already settled");
    if (dispute.closedNoRuling) fail("dispute closed with no ruling");
    // The new draw epoch is computed before any state changes, so a
    // refused sum leaves the dispute exactly as it was.
    const newDrawEpoch = safeEpochSum(this.epoch, drawDelay);
    dispute.failedJurors = dispute.panel.filter((key) => !dispute.reveals.has(key));
    dispute.panel = [];
    dispute.commits = new Map();
    dispute.reveals = new Map();
    dispute.round += 1;
    dispute.drawEpoch = newDrawEpoch;
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
    return panel.filter((key) => {
      const vote = reveals.get(key);
      return vote !== outcome && vote !== VOTE_REFUSE;
    });
  }

  slashKeys(keys) {
    let total = 0n;
    for (const key of keys) {
      const bond = this.bonds.get(key) || 0n;
      const slash = bond < BigInt(SLASH) ? bond : BigInt(SLASH);
      this.bonds.set(key, bond - slash);
      total += slash;
    }
    this.feeVault += total;
    return total;
  }

  recordClaims(dispute, keys, feeTotal, slashTotal) {
    const feeShares = keys.length
      ? splitLargestRemainder(feeTotal, keys.map(() => 1))
      : [];
    const slashShares = keys.length
      ? splitLargestRemainder(slashTotal, keys.map(() => 1))
      : [];
    keys.forEach((key, i) => {
      const claimAmount = feeShares[i] + slashShares[i];
      if (claimAmount > 0n) {
        const existing = dispute.claims.get(key);
        dispute.claims.set(key, {
          amount: (existing?.amount || 0n) + claimAmount,
          claimed: existing?.claimed || false,
        });
      }
    });
  }

  // Settlement is atomic: every precondition is checked and every amount
  // computed before state moves, and if anything still fails, the whole
  // call rolls back to the snapshot taken on entry, so a failing
  // settlement leaves the simulation exactly as it was.
  settle(disputeId) {
    const snapshot = this.snapshotState();
    try {
      return this.settleInner(disputeId);
    } catch (err) {
      this.restoreState(snapshot);
      throw err;
    }
  }

  settleInner(disputeId) {
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
    // The reveal window must have passed before anyone can be slashed
    // for not revealing: a juror inside their window is not an offender.
    if (this.epoch <= dispute.revealDeadline) fail("reveal window has not passed yet");
    if (dispute.appealed && this.epoch <= dispute.appealRevealDeadline) {
      fail("appeal reveal window has not passed yet");
    }
    const escrow = this.escrows.get(dispute.escrowId);
    const amounts = settleAmounts(dispute.principal, outcome, first.count);
    const fees = amounts.fees;
    // A split of nothing is a pay, not a split: when the juror fees
    // equal the principal, the post-fee principal is 0, so a split
    // would route (0, principal), which the escrow's split positivity
    // check refuses. The outcome the escrow sees is pay, the same
    // conversion the template makes before it orders the ruling.
    if (outcome === VOTE_SPLIT && dispute.principal - fees === 0n) {
      outcome = VOTE_PAY;
    }
    // One call returns both buckets: the payee amount and the gross
    // payer amount. The court takes the fees out of the gross payer
    // bucket before paying the payer the net amount.
    const [payeeMoved, payerMovedGross] = escrow.executeRuling(
      disputeId,
      outcome,
      [amounts.payeeAmount, amounts.payerAmount],
      this.courtId,
    );
    if (payeeMoved !== amounts.payeeAmount) fail("escrow paid a different amount than ruled");
    if (payerMovedGross !== amounts.payerAmount) {
      fail("escrow paid a different payer amount than ruled");
    }
    this.feeVault += fees;
    const payerMoved = payerMovedGross - fees;
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
    let leftoverAppealFee = 0n;
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
      const appealFeeTotal = BigInt(appealMajority.length * JUROR_FEE);
      leftoverAppealFee = BigInt(APPEAL_FEE) - appealFeeTotal;
      this.recordClaims(dispute, appealMajority, appealFeeTotal, appealSlash);
    }
    this.recordClaims(dispute, firstMajority, fees, firstSlash);
    if (leftoverAppealFee > 0n) {
      this.fund(dispute.appellantAccount, leftoverAppealFee);
      this.feeVault -= leftoverAppealFee;
    }
    dispute.outcome = outcome;
    dispute.payeeAmount = amounts.payeeAmount;
    dispute.payerAmount = payerMoved;
    dispute.feesPaid = fees;
    dispute.settled = true;
    return {
      outcome,
      payeeAmount: amounts.payeeAmount,
      payerAmount: payerMoved,
      payerAmountGross: payerMovedGross,
      fees,
      leftoverAppealFee,
    };
  }

  openAppeal(disputeId, caller, fee, drawDelay) {
    const dispute = this.dispute(disputeId);
    validDelay(drawDelay);
    if (caller !== dispute.payer && caller !== dispute.payee) {
      fail("only a party to the dispute can appeal");
    }
    const feeAmount = amount(fee);
    if (feeAmount !== BigInt(APPEAL_FEE)) fail("appeal fee must be exactly APPEAL_FEE");
    if (!dispute.drawn) fail("jury not drawn for this dispute");
    if (dispute.settled) fail("dispute already settled");
    if (dispute.closedNoRuling) fail("dispute closed with no ruling");
    if (dispute.appealed) fail("appeal already opened");
    const first = this.firstOutcome(dispute);
    if (first.vote === null) fail("first round has no majority ruling to appeal");
    if (this.jurors.size < APPEAL_PANEL_SIZE) {
      fail("not enough eligible jurors to draw an appeal panel");
    }
    if (this.balance(caller) < feeAmount) fail("insufficient balance to post the appeal fee");
    // The draw epoch is computed before any state changes, so a
    // refused sum leaves the fee with the caller and the appeal unopened.
    const appealDrawEpoch = safeEpochSum(this.epoch, drawDelay);
    this.balances.set(caller, this.balance(caller) - feeAmount);
    this.feeVault += feeAmount;
    dispute.appealed = true;
    dispute.appealFee = feeAmount;
    dispute.appellantAccount = caller;
    dispute.appealDrawEpoch = appealDrawEpoch;
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
    // The deadlines are computed before any state changes, so a
    // refused sum leaves the dispute exactly as it was.
    const appealRevealDeadline = safeEpochSum(dispute.appealDrawEpoch, REVEAL_WINDOW);
    const appealCommitDeadline = safeEpochSum(dispute.appealDrawEpoch, 1);
    dispute.appealDrawHash = drawHash;
    dispute.appealPanel = indexes.map((i) => pool[i]);
    dispute.appealRevealDeadline = appealRevealDeadline;
    dispute.appealCommitDeadline = appealCommitDeadline;
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
    if (dispute.appealReschedules >= MAX_RESCHEDULES) {
      fail("appeal draw reschedule limit reached");
    }
    dispute.appealDrawEpoch = safeEpochSum(this.epoch, 1);
    dispute.appealReschedules += 1;
    return dispute.appealDrawEpoch;
  }

  commitAppealVote(disputeId, juror, vote, salt) {
    const dispute = this.dispute(disputeId);
    if (!dispute.appealed) fail("no appeal opened for this dispute");
    if (!dispute.appealDrawn) fail("appeal jury not drawn");
    if (dispute.settled) fail("dispute already settled");
    if (dispute.closedNoRuling) fail("dispute closed with no ruling");
    if (this.epoch > dispute.appealCommitDeadline) fail("appeal commit window has passed");
    if (!dispute.appealPanel.includes(juror)) fail("juror not drawn for this appeal");
    if (dispute.appealCommits.has(juror)) fail("juror already committed an appeal vote");
    validVote(vote);
    const commitment = sha256Hex(voteCommitmentBytes(vote, salt, disputeId, juror));
    dispute.appealCommits.set(juror, commitment);
    return commitment;
  }

  revealAppealVote(disputeId, juror, vote, salt) {
    const dispute = this.dispute(disputeId);
    if (!dispute.appealed) fail("no appeal opened for this dispute");
    if (!dispute.appealDrawn) fail("appeal jury not drawn");
    if (dispute.settled) fail("dispute already settled");
    if (dispute.closedNoRuling) fail("dispute closed with no ruling");
    if (this.epoch <= dispute.appealCommitDeadline) fail("appeal commit window has not ended");
    if (this.epoch > dispute.appealRevealDeadline) fail("appeal reveal window has passed");
    validVote(vote);
    const stored = dispute.appealCommits.get(juror);
    if (stored === undefined) fail("no appeal vote commitment from this juror");
    if (dispute.appealReveals.has(juror)) fail("juror already revealed an appeal vote");
    const expected = sha256Hex(voteCommitmentBytes(vote, salt, disputeId, juror));
    if (stored !== expected) fail("reveal does not match the commitment");
    dispute.appealReveals.set(juror, vote);
    return vote;
  }

  // Anyone can call a replacement: an incomplete round must not
  // depend on a party submitting a transaction.
  replaceAppealJury(disputeId, caller, drawDelay) {
    const dispute = this.dispute(disputeId);
    validDelay(drawDelay);
    if (!dispute.appealed) fail("no appeal opened for this dispute");
    if (!dispute.appealDrawn) fail("appeal jury not drawn");
    if (dispute.appealReplacementUsed) fail("appeal replacement already used");
    if (this.epoch <= dispute.appealRevealDeadline) {
      fail("appeal reveal window has not passed yet");
    }
    if (dispute.settled) fail("dispute already settled");
    if (dispute.closedNoRuling) fail("dispute closed with no ruling");
    // The new draw epoch is computed before any state changes, so a
    // refused sum leaves the dispute exactly as it was.
    const newAppealDrawEpoch = safeEpochSum(this.epoch, drawDelay);
    dispute.appealFailedJurors = dispute.appealPanel.filter(
      (key) => !dispute.appealReveals.has(key),
    );
    dispute.appealPanel = [];
    dispute.appealCommits = new Map();
    dispute.appealReveals = new Map();
    dispute.appealRound += 1;
    dispute.appealDrawEpoch = newAppealDrawEpoch;
    dispute.appealDrawn = false;
    dispute.appealReplacementUsed = true;
    return dispute.appealDrawEpoch;
  }

  // The no-ruling close is atomic, the same way settlement is: every
  // precondition is checked and every amount computed before state
  // moves, and if anything still fails, the whole call rolls back to
  // the snapshot taken on entry, so a failing close leaves the
  // simulation exactly as it was.
  closeNoRuling(disputeId) {
    const snapshot = this.snapshotState();
    try {
      return this.closeNoRulingInner(disputeId);
    } catch (err) {
      this.restoreState(snapshot);
      throw err;
    }
  }

  closeNoRulingInner(disputeId) {
    const dispute = this.dispute(disputeId);
    if (dispute.settled) fail("dispute already settled");
    if (dispute.closedNoRuling) fail("dispute already closed with no ruling");
    // A round is finished when it is fully revealed. A replacement
    // round is also finished once its own reveal window has passed with
    // no majority, so the replacement round always gets its full window
    // to vote before the close can run.
    const roundFinished =
      dispute.drawn &&
      (dispute.reveals.size === dispute.panel.length ||
        (dispute.replacementUsed && this.epoch > dispute.revealDeadline));
    const appealFinished =
      dispute.appealDrawn &&
      (dispute.appealReveals.size === dispute.appealPanel.length ||
        (dispute.appealReplacementUsed && this.epoch > dispute.appealRevealDeadline));
    // A first round that already ruled is final once its reveal window
    // has passed: no more first-round reveals can land after the
    // deadline, so its majority cannot change. A close under appeal
    // needs that as well as the appeal round finished, so both rounds
    // are done before the dispute exits.
    const firstRoundFinal =
      dispute.drawn &&
      (dispute.reveals.size === dispute.panel.length ||
        this.epoch > dispute.revealDeadline);
    // A draw that never executed can never execute once its
    // reschedules are spent and its final locked epoch has passed. The
    // dispute then closes the same way instead of staying frozen.
    const neverDrawn =
      !dispute.drawn &&
      dispute.reschedules >= MAX_RESCHEDULES &&
      this.epoch > dispute.drawEpoch;
    const appealNeverDrawn =
      dispute.appealed &&
      !dispute.appealDrawn &&
      dispute.appealReschedules >= MAX_RESCHEDULES &&
      this.epoch > dispute.appealDrawEpoch;
    const firstNoMajority = this.firstOutcome(dispute).vote === null;
    const appealNoMajority = this.appealOutcome(dispute).vote === null;
    if (dispute.appealed) {
      if (!firstRoundFinal) fail("first round can still reach a ruling");
      if (!(appealNoMajority && (appealFinished || appealNeverDrawn))) {
        fail("appeal round can still reach a ruling");
      }
    } else if (!(firstNoMajority && roundFinished) && !neverDrawn) {
      fail("first round can still reach a ruling");
    }
    const escrow = this.escrows.get(dispute.escrowId);
    const principal = escrow.vault;
    // One call returns both buckets: a payee amount of 0 and the whole
    // principal as the gross payer amount. No fees are taken.
    const [payeeMoved, payerMoved] = escrow.executeRuling(
      disputeId,
      NO_OUTCOME,
      [0n, principal],
      this.courtId,
    );
    if (payeeMoved !== 0n) fail("no-ruling payee amount must be 0");
    if (payerMoved !== principal) fail("no-ruling payer amount must be the whole principal");
    this.fund(dispute.payer, payerMoved);
    const rulingPanel = dispute.appealed ? dispute.appealPanel : dispute.panel;
    const rulingReveals = dispute.appealed ? dispute.appealReveals : dispute.reveals;
    // Jurors who voted to refuse are neither paid nor slashed: they are
    // left out of the payable set, and they revealed, so they are left
    // out of the slash set too.
    const revealedKeys = [...rulingReveals.keys()];
    const payableKeys = [...rulingReveals.entries()]
      .filter(([, vote]) => vote !== VOTE_REFUSE)
      .map(([key]) => key);
    const failedJurors = dispute.appealed
      ? dispute.appealFailedJurors
      : dispute.failedJurors;
    let slashTotal = 0n;
    if (payableKeys.length > 0) {
      const slashKeys = rulingPanel.filter((key) => !revealedKeys.includes(key));
      for (const key of failedJurors) {
        if (!slashKeys.includes(key)) slashKeys.push(key);
      }
      slashTotal = this.slashKeys(slashKeys);
      if (slashTotal > 0n) {
        const shares = splitLargestRemainder(slashTotal, payableKeys.map(() => 1));
        payableKeys.forEach((key, i) => {
          if (shares[i] > 0n) {
            dispute.claims.set(key, { amount: shares[i], claimed: false });
          }
        });
      }
    }
    if (dispute.appealed && dispute.appealFee > 0n) {
      this.fund(dispute.appellantAccount, dispute.appealFee);
      this.feeVault -= dispute.appealFee;
    }
    dispute.closedNoRuling = true;
    dispute.outcome = NO_OUTCOME;
    dispute.payeeAmount = 0n;
    dispute.payerAmount = payerMoved;
    dispute.feesPaid = 0n;
    return { payerAmount: payerMoved, slashTotal };
  }

  claimFee(disputeId, juror) {
    const dispute = this.dispute(disputeId);
    const claim = dispute.claims.get(juror);
    if (!claim) fail("no fee claim for this juror");
    if (claim.claimed) fail("fee already claimed");
    if (claim.amount <= 0n) fail("no fee owed for this dispute");
    if (this.feeVault < claim.amount) fail("fee vault cannot cover the claim");
    this.feeVault -= claim.amount;
    claim.claimed = true;
    this.fund(juror, claim.amount);
    return claim.amount;
  }

  disputeClaim(disputeId, juror) {
    const dispute = this.dispute(disputeId);
    return dispute.claims.get(juror)?.amount || 0n;
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
