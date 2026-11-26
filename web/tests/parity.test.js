// Parity tests: the simulation in src/simulation.js agrees with the
// mirror in src/verdict.js and with the template rules in
// templates/verdict/src/lib.rs at 278344b, on every rule and every
// rejection message. Run from web/:
//   node --test tests/parity.test.js
// The engine suite is the other half of the proof:
//   cargo test (from the repo root, with the build target set)
// Every template rule and rejection message covered here has a matching
// engine test in templates/verdict/tests/.

import { describe, test } from "node:test";
import assert from "node:assert/strict";
import { SimCourt, SimEscrow, NO_OUTCOME } from "../src/simulation.js";
import {
  PANEL_SIZE,
  APPEAL_PANEL_SIZE,
  JUROR_FEE,
  BOND_FLOOR,
  APPEAL_FEE,
  SLASH,
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
  sha256Hex,
} from "../src/verdict.js";
import { SAMPLE } from "../src/sample-data.js";

const TOTAL = 500000;
const DRAW_DELAY = 2;

function jurorKey(i) {
  return sha256Hex(`sim-juror-${i}`);
}

function salt(i) {
  return sha256Hex(`sim-salt-${i}`);
}

// A court with `count` registered jurors, each funded and bonded.
function makeCourt(count = 7) {
  const court = new SimCourt();
  for (let i = 0; i < count; i += 1) {
    court.fund(jurorKey(i), 100000);
    court.registerJuror(jurorKey(i), BOND_FLOOR);
  }
  return court;
}

// A funded escrow for a 500,000 principal, opened at epoch 0.
function openFunded(court, principal = TOTAL) {
  court.fund("payer", 1000000);
  court.fund("payee", 0);
  court.createEscrow("escrow-0", "payer", "payee", principal);
  return court.openDispute("escrow-0", DRAW_DELAY);
}

function drawAt(court, disputeId) {
  const dispute = court.dispute(disputeId);
  court.setEpoch(dispute.drawEpoch);
  return court.drawJury(disputeId);
}

function rulePanel(court, disputeId, panel, votes) {
  panel.forEach((juror, i) => {
    court.commitVote(disputeId, juror, votes[i], salt(i));
    court.revealVote(disputeId, juror, votes[i], salt(i));
  });
}

function ruleAppealPanel(court, disputeId, panel, votes) {
  panel.forEach((juror, i) => {
    court.commitAppealVote(disputeId, juror, votes[i], salt(i + 20));
    court.revealAppealVote(disputeId, juror, votes[i], salt(i + 20));
  });
}

function disputeWithRuling(court, votes) {
  const disputeId = openFunded(court);
  const { panel } = drawAt(court, disputeId);
  rulePanel(court, disputeId, panel, votes);
  return { disputeId, panel };
}

function disputeWithAppealDrawn(court, firstVotes) {
  const { disputeId, panel } = disputeWithRuling(court, firstVotes);
  court.openAppeal(disputeId, "payer", APPEAL_FEE, DRAW_DELAY);
  const dispute = court.dispute(disputeId);
  court.setEpoch(dispute.appealDrawEpoch);
  const { panel: appealPanel } = court.drawAppealJury(disputeId);
  return { disputeId, panel, appealPanel };
}

// Settle once the reveal window has passed, as the template requires:
// the epoch moves to the latest reveal deadline first, the first-round
// deadline, or the appeal deadline when the dispute is under appeal.
function settleAtDeadline(court, disputeId) {
  const dispute = court.dispute(disputeId);
  let epoch = dispute.revealDeadline;
  if (dispute.appealed) epoch = Math.max(epoch, dispute.appealRevealDeadline);
  court.setEpoch(epoch);
  return court.settle(disputeId);
}

describe("constants match the template", () => {
  test("panel sizes, fees, bond floor, delay, window, and domains", () => {
    assert.equal(PANEL_SIZE, 3);
    assert.equal(APPEAL_PANEL_SIZE, 7);
    assert.equal(JUROR_FEE, 5000);
    assert.equal(BOND_FLOOR, 30000);
    assert.equal(APPEAL_FEE, 35000);
    assert.equal(SLASH, 5000);
    assert.equal(MIN_DRAW_DELAY, 1);
    assert.equal(REVEAL_WINDOW, 2);
    assert.equal(DRAW_DOMAIN_FIRST, 0);
    assert.equal(DRAW_DOMAIN_APPEAL, 1);
  });
});

describe("panel derivation agrees with the mirror", () => {
  test("first-round draw equals derivePanel with domain 0", () => {
    const court = makeCourt();
    const disputeId = openFunded(court);
    const dispute = court.dispute(disputeId);
    const hash = sha256Hex("parity-draw-hash");
    court.setEpochHash(dispute.drawEpoch, hash);
    court.setEpoch(dispute.drawEpoch);
    const { panel } = court.drawJury(disputeId);
    const expected = derivePanel(
      dispute.eligiblePool,
      hash,
      disputeId,
      0,
      DRAW_DOMAIN_FIRST,
      PANEL_SIZE,
    ).map((i) => dispute.eligiblePool[i]);
    assert.deepEqual(panel, expected);
  });

  test("appeal draw equals derivePanel with domain 1 and seven jurors", () => {
    const court = makeCourt();
    const { disputeId } = disputeWithRuling(court, [VOTE_PAY, VOTE_PAY, VOTE_REFUND]);
    court.openAppeal(disputeId, "payer", APPEAL_FEE, DRAW_DELAY);
    const dispute = court.dispute(disputeId);
    const hash = sha256Hex("parity-appeal-hash");
    court.setEpochHash(dispute.appealDrawEpoch, hash);
    court.setEpoch(dispute.appealDrawEpoch);
    const { panel } = court.drawAppealJury(disputeId);
    const pool = [...court.jurors.keys()];
    const expected = derivePanel(
      pool,
      hash,
      disputeId,
      0,
      DRAW_DOMAIN_APPEAL,
      APPEAL_PANEL_SIZE,
    ).map((i) => pool[i]);
    assert.deepEqual(panel, expected);
  });

  test("appeal panel differs from the first round with the same hash", () => {
    const court = makeCourt();
    const disputeId = openFunded(court);
    const hash = sha256Hex("parity-same-hash");
    const dispute = court.dispute(disputeId);
    court.setEpochHash(dispute.drawEpoch, hash);
    court.setEpoch(dispute.drawEpoch);
    const { panel: firstPanel } = court.drawJury(disputeId);
    rulePanel(court, disputeId, firstPanel, [VOTE_PAY, VOTE_PAY, VOTE_REFUND]);
    court.openAppeal(disputeId, "payer", APPEAL_FEE, DRAW_DELAY);
    const after = court.dispute(disputeId);
    court.setEpochHash(after.appealDrawEpoch, hash);
    court.setEpoch(after.appealDrawEpoch);
    const { panel: appealPanel } = court.drawAppealJury(disputeId);
    assert.equal(appealPanel.length, 7);
    assert.notDeepEqual(appealPanel.slice(0, 3), firstPanel);
    assert.notDeepEqual(appealPanel, firstPanel);
  });
});

describe("majority rule agrees with the mirror", () => {
  test("one revealed vote for pay returns no ruling in both", () => {
    assert.deepEqual(majorityOutcome([VOTE_PAY], PANEL_SIZE), { vote: null, count: 0 });
    const court = makeCourt(3);
    const disputeId = openFunded(court);
    const { panel } = drawAt(court, disputeId);
    rulePanel(court, disputeId, panel.slice(0, 1), [VOTE_PAY]);
    assert.throws(() => court.settle(disputeId), /first round has no majority ruling/);
  });

  test("two refuse votes return no ruling in both", () => {
    assert.deepEqual(majorityOutcome([VOTE_REFUSE, VOTE_REFUSE], PANEL_SIZE), {
      vote: null,
      count: 0,
    });
    const court = makeCourt(3);
    const disputeId = openFunded(court);
    const { panel } = drawAt(court, disputeId);
    rulePanel(court, disputeId, panel, [VOTE_REFUSE, VOTE_REFUSE, VOTE_PAY]);
    assert.throws(() => court.settle(disputeId), /first round has no majority ruling/);
  });

  test("one of each vote returns no ruling in both", () => {
    assert.deepEqual(majorityOutcome([VOTE_PAY, VOTE_REFUND, VOTE_SPLIT], PANEL_SIZE), {
      vote: null,
      count: 1,
    });
    const court = makeCourt(3);
    const disputeId = openFunded(court);
    const { panel } = drawAt(court, disputeId);
    rulePanel(court, disputeId, panel, [VOTE_PAY, VOTE_REFUND, VOTE_SPLIT]);
    assert.throws(() => court.settle(disputeId), /first round has no majority ruling/);
  });

  test("three reveals on a panel of seven return no ruling: the quorum is not met", () => {
    assert.deepEqual(
      majorityOutcome([VOTE_PAY, VOTE_PAY, VOTE_PAY], APPEAL_PANEL_SIZE),
      { vote: null, count: 0 },
    );
    const court = makeCourt();
    const { disputeId, appealPanel } = disputeWithAppealDrawn(court, [
      VOTE_PAY,
      VOTE_PAY,
      VOTE_REFUND,
    ]);
    ruleAppealPanel(court, disputeId, appealPanel.slice(0, 3), [
      VOTE_PAY,
      VOTE_PAY,
      VOTE_PAY,
    ]);
    assert.throws(() => court.settle(disputeId), /appeal round has no majority ruling/);
  });

  test("two of three pay rules pay, four of seven refund rules refund", () => {
    assert.deepEqual(majorityOutcome([VOTE_PAY, VOTE_PAY, VOTE_REFUND], PANEL_SIZE), {
      vote: VOTE_PAY,
      count: 2,
    });
    assert.deepEqual(
      majorityOutcome(
        [VOTE_REFUND, VOTE_REFUND, VOTE_REFUND, VOTE_REFUND, VOTE_PAY, VOTE_PAY, VOTE_PAY],
        APPEAL_PANEL_SIZE,
      ),
      { vote: VOTE_REFUND, count: 4 },
    );
  });

  test("three pay of four revealed on a panel of seven returns no ruling in both", () => {
    assert.deepEqual(
      majorityOutcome([VOTE_PAY, VOTE_PAY, VOTE_PAY, VOTE_REFUND], APPEAL_PANEL_SIZE),
      { vote: null, count: 3 },
    );
    const court = makeCourt();
    const { disputeId, appealPanel } = disputeWithAppealDrawn(court, [
      VOTE_PAY,
      VOTE_PAY,
      VOTE_REFUND,
    ]);
    ruleAppealPanel(court, disputeId, appealPanel.slice(0, 4), [
      VOTE_PAY,
      VOTE_PAY,
      VOTE_PAY,
      VOTE_REFUND,
    ]);
    assert.throws(() => court.settle(disputeId), /appeal round has no majority ruling/);
  });

  test("four matching pay votes on a panel of seven rule pay in both", () => {
    assert.deepEqual(majorityOutcome([VOTE_PAY, VOTE_PAY, VOTE_PAY, VOTE_PAY], APPEAL_PANEL_SIZE), {
      vote: VOTE_PAY,
      count: 4,
    });
    const court = makeCourt();
    const { disputeId, appealPanel } = disputeWithAppealDrawn(court, [
      VOTE_PAY,
      VOTE_PAY,
      VOTE_REFUND,
    ]);
    ruleAppealPanel(court, disputeId, appealPanel.slice(0, 4), [
      VOTE_PAY,
      VOTE_PAY,
      VOTE_PAY,
      VOTE_PAY,
    ]);
    const result = settleAtDeadline(court, disputeId);
    assert.equal(result.outcome, VOTE_PAY);
    assert.equal(result.payeeAmount, 490000n);
    assert.equal(court.escrows.get("escrow-0").vault, 0n);
  });
});

describe("settlement matrix", () => {
  test("pay majority pays fees first: payee 490,000, claims 7,500 each", () => {
    const court = makeCourt(3);
    const { disputeId, panel } = disputeWithRuling(court, [VOTE_PAY, VOTE_PAY, VOTE_REFUND]);
    const payerBefore = court.balance("payer");
    const payeeBefore = court.balance("payee");
    const result = settleAtDeadline(court, disputeId);
    assert.deepEqual(result, {
      outcome: VOTE_PAY,
      payeeAmount: 490000n,
      payerAmount: 0n,
      fees: 10000n,
      leftoverAppealFee: 0n,
    });
    assert.deepEqual(settleAmounts(TOTAL, VOTE_PAY, 2), {
      fees: 10000,
      payeeAmount: 490000,
      payerAmount: 0,
    });
    assert.equal(court.balance("payee"), payeeBefore + 490000n);
    assert.equal(court.balance("payer"), payerBefore);
    assert.equal(court.dispute(disputeId).escrowId && court.escrows.get("escrow-0").vault, 0n);
    assert.equal(court.disputeClaim(disputeId, panel[0]), 7500n);
    assert.equal(court.disputeClaim(disputeId, panel[1]), 7500n);
    assert.equal(court.disputeClaim(disputeId, panel[2]), 0n);
    assert.equal(court.bonds.get(panel[2]), BigInt(BOND_FLOOR - JUROR_FEE));
    assert.equal(490000n + 0n + 10000n, BigInt(TOTAL));
  });

  test("refund majority: payer 290,000 on a 300,000 principal", () => {
    const court = makeCourt(3);
    const disputeId = openFunded(court, 300000);
    const payerBefore = court.balance("payer");
    const { panel } = drawAt(court, disputeId);
    rulePanel(court, disputeId, panel, [VOTE_REFUND, VOTE_REFUND, VOTE_PAY]);
    const result = settleAtDeadline(court, disputeId);
    assert.equal(result.payeeAmount, 0n);
    assert.equal(result.payerAmount, 290000n);
    assert.equal(result.fees, 10000n);
    assert.equal(court.balance("payer"), payerBefore + 290000n);
    assert.equal(0n + 290000n + 10000n, 300000n);
  });

  test("split even: 245,000 each", () => {
    const court = makeCourt(3);
    const { disputeId } = disputeWithRuling(court, [VOTE_SPLIT, VOTE_SPLIT, VOTE_PAY]);
    const result = settleAtDeadline(court, disputeId);
    assert.deepEqual(settleAmounts(TOTAL, VOTE_SPLIT, 2), {
      fees: 10000,
      payeeAmount: 245000,
      payerAmount: 245000,
    });
    assert.equal(result.payeeAmount, 245000n);
    assert.equal(result.payerAmount, 245000n);
    assert.equal(245000n + 245000n + 10000n, BigInt(TOTAL));
  });

  test("split odd dust: 45,001 to the payee, 45,000 to the payer", () => {
    const court = makeCourt(3);
    const disputeId = openFunded(court, 100001);
    const { panel } = drawAt(court, disputeId);
    rulePanel(court, disputeId, panel, [VOTE_SPLIT, VOTE_SPLIT, VOTE_PAY]);
    const result = settleAtDeadline(court, disputeId);
    assert.deepEqual(settleAmounts(100001, VOTE_SPLIT, 2), {
      fees: 10000,
      payeeAmount: 45001,
      payerAmount: 45000,
    });
    assert.equal(result.payeeAmount, 45001n);
    assert.equal(result.payerAmount, 45000n);
    assert.equal(45001n + 45000n + 10000n, 100001n);
  });

  test("fees are capped at a small principal", () => {
    const court = makeCourt(3);
    const disputeId = openFunded(court, 10000);
    const { panel } = drawAt(court, disputeId);
    rulePanel(court, disputeId, panel, [VOTE_PAY, VOTE_PAY, VOTE_REFUND]);
    const result = settleAtDeadline(court, disputeId);
    assert.deepEqual(result, {
      outcome: VOTE_PAY,
      payeeAmount: 0n,
      payerAmount: 0n,
      fees: 10000n,
      leftoverAppealFee: 0n,
    });
    assert.equal(0n + 0n + 10000n, 10000n);
  });

  test("settle before quorum is rejected", () => {
    const court = makeCourt(3);
    const disputeId = openFunded(court);
    const { panel } = drawAt(court, disputeId);
    rulePanel(court, disputeId, panel.slice(0, 1), [VOTE_PAY]);
    assert.throws(() => court.settle(disputeId), /first round has no majority ruling/);
  });

  test("double settlement is rejected", () => {
    const court = makeCourt(3);
    const { disputeId } = disputeWithRuling(court, [VOTE_PAY, VOTE_PAY, VOTE_REFUND]);
    settleAtDeadline(court, disputeId);
    assert.throws(() => court.settle(disputeId), /dispute already settled/);
  });

  test("claim_fee pays the majority once and refuses the minority and a second claim", () => {
    const court = makeCourt(3);
    const { disputeId, panel } = disputeWithRuling(court, [VOTE_PAY, VOTE_PAY, VOTE_REFUND]);
    settleAtDeadline(court, disputeId);
    assert.equal(court.claimFee(disputeId, panel[0]), 7500n);
    assert.throws(() => court.claimFee(disputeId, panel[0]), /fee already claimed/);
    assert.throws(() => court.claimFee(disputeId, panel[2]), /no fee claim for this juror/);
  });
});

describe("replacement draw", () => {
  test("replacement is rejected before the reveal deadline", () => {
    const court = makeCourt(3);
    const disputeId = openFunded(court);
    const { panel } = drawAt(court, disputeId);
    rulePanel(court, disputeId, panel.slice(0, 1), [VOTE_PAY]);
    assert.equal(court.dispute(disputeId).revealDeadline, DRAW_DELAY + REVEAL_WINDOW);
    assert.throws(
      () => court.replaceJury(disputeId, "payer", DRAW_DELAY),
      /reveal window has not passed yet/,
    );
  });

  test("replacement is rejected for a stranger", () => {
    const court = makeCourt(3);
    const disputeId = openFunded(court);
    const { panel } = drawAt(court, disputeId);
    rulePanel(court, disputeId, panel.slice(0, 1), [VOTE_PAY]);
    const dispute = court.dispute(disputeId);
    court.setEpoch(dispute.revealDeadline);
    assert.throws(
      () => court.replaceJury(disputeId, "stranger", DRAW_DELAY),
      /only a party to the dispute can replace the jury/,
    );
  });

  test("replacement is allowed after the deadline and recovers after a no-show", () => {
    const court = makeCourt(3);
    const disputeId = openFunded(court);
    const { panel } = drawAt(court, disputeId);
    rulePanel(court, disputeId, panel.slice(0, 1), [VOTE_PAY]);
    const dispute = court.dispute(disputeId);
    court.setEpoch(dispute.revealDeadline);
    const newEpoch = court.replaceJury(disputeId, "payer", DRAW_DELAY);
    assert.equal(newEpoch, dispute.revealDeadline + DRAW_DELAY);
    assert.equal(court.dispute(disputeId).replacementUsed, true);
    court.setEpoch(newEpoch);
    const redrawn = court.drawJury(disputeId);
    assert.equal(redrawn.panel.length, 3);
    rulePanel(court, disputeId, redrawn.panel, [VOTE_PAY, VOTE_PAY, VOTE_REFUND]);
    settleAtDeadline(court, disputeId);
    assert.equal(court.disputeClaim(disputeId, redrawn.panel[0]), 10000n);
    assert.equal(court.disputeClaim(disputeId, redrawn.panel[1]), 10000n);
    assert.equal(court.escrows.get("escrow-0").vault, 0n);
  });

  test("a second replacement is rejected", () => {
    const court = makeCourt(3);
    const disputeId = openFunded(court);
    const { panel } = drawAt(court, disputeId);
    rulePanel(court, disputeId, panel.slice(0, 1), [VOTE_PAY]);
    const dispute = court.dispute(disputeId);
    court.setEpoch(dispute.revealDeadline);
    court.replaceJury(disputeId, "payer", DRAW_DELAY);
    court.setEpoch(dispute.drawEpoch);
    court.drawJury(disputeId);
    rulePanel(court, disputeId, court.dispute(disputeId).panel.slice(0, 1), [VOTE_PAY]);
    court.setEpoch(court.dispute(disputeId).revealDeadline);
    assert.throws(
      () => court.replaceJury(disputeId, "payer", DRAW_DELAY),
      /replacement already used/,
    );
  });
});

describe("no-ruling fallback", () => {
  test("returns the whole principal to the payer and ends the escrow", () => {
    const court = makeCourt(3);
    const disputeId = openFunded(court);
    const payerBefore = court.balance("payer");
    const { panel } = drawAt(court, disputeId);
    rulePanel(court, disputeId, panel.slice(0, 1), [VOTE_PAY]);
    const dispute = court.dispute(disputeId);
    court.setEpoch(dispute.revealDeadline);
    court.replaceJury(disputeId, "payer", DRAW_DELAY);
    court.setEpoch(court.dispute(disputeId).drawEpoch);
    const redrawn = court.drawJury(disputeId);
    rulePanel(court, disputeId, redrawn.panel.slice(0, 1), [VOTE_REFUND]);
    // The replacement round must finish first: its window runs to the
    // new reveal deadline, and only then can the close run.
    court.setEpoch(court.dispute(disputeId).revealDeadline);
    const result = court.closeNoRuling(disputeId);
    assert.equal(result.payerAmount, BigInt(TOTAL));
    assert.equal(court.balance("payer"), payerBefore + BigInt(TOTAL));
    assert.equal(court.escrows.get("escrow-0").vault, 0n);
    assert.equal(court.dispute(disputeId).closedNoRuling, true);
    assert.throws(() => court.settle(disputeId), /dispute closed with no ruling/);
  });

  test("refuse votes block settlement and close, with bonds untouched", () => {
    const court = makeCourt(3);
    const disputeId = openFunded(court);
    const { panel } = drawAt(court, disputeId);
    rulePanel(court, disputeId, panel, [VOTE_REFUSE, VOTE_REFUSE, VOTE_PAY]);
    assert.throws(() => court.settle(disputeId), /first round has no majority ruling/);
    court.closeNoRuling(disputeId);
    for (const key of panel) {
      assert.equal(court.bonds.get(key), BigInt(BOND_FLOOR));
    }
    assert.equal(court.balance("payer"), 1000000n);
  });

  test("close is rejected while the round can still reach a ruling", () => {
    const court = makeCourt(3);
    const disputeId = openFunded(court);
    const { panel } = drawAt(court, disputeId);
    rulePanel(court, disputeId, panel, [VOTE_PAY, VOTE_PAY, VOTE_REFUND]);
    assert.throws(() => court.closeNoRuling(disputeId), /first round can still reach a ruling/);
  });
});

describe("draw delay and reschedule", () => {
  test("draw delay 0 is rejected in all four draw methods", () => {
    const court = makeCourt();
    assert.throws(() => court.openDispute("escrow-0", 0), /draw delay must be at least one epoch/);
    const disputeId = openFunded(court);
    const { panel } = drawAt(court, disputeId);
    rulePanel(court, disputeId, panel.slice(0, 1), [VOTE_PAY]);
    const dispute = court.dispute(disputeId);
    court.setEpoch(dispute.revealDeadline);
    assert.throws(
      () => court.replaceJury(disputeId, "payer", 0),
      /draw delay must be at least one epoch/,
    );
    rulePanel(court, disputeId, panel.slice(1, 3), [VOTE_PAY, VOTE_REFUND]);
    assert.throws(
      () => court.openAppeal(disputeId, "payer", APPEAL_FEE, 0),
      /draw delay must be at least one epoch/,
    );
    court.openAppeal(disputeId, "payer", APPEAL_FEE, DRAW_DELAY);
    const after = court.dispute(disputeId);
    court.setEpoch(after.appealDrawEpoch);
    const { panel: appealPanel } = court.drawAppealJury(disputeId);
    ruleAppealPanel(court, disputeId, appealPanel.slice(0, 3), [
      VOTE_REFUND,
      VOTE_REFUND,
      VOTE_REFUND,
    ]);
    court.setEpoch(after.appealRevealDeadline);
    assert.throws(
      () => court.replaceAppealJury(disputeId, "payer", 0),
      /draw delay must be at least one epoch/,
    );
  });

  test("a reschedule never lands on the current epoch", () => {
    const court = makeCourt(3);
    const disputeId = openFunded(court);
    court.setEpoch(DRAW_DELAY + 1);
    const newEpoch = court.rescheduleDraw(disputeId);
    assert.equal(newEpoch, DRAW_DELAY + 2);
    court.setEpoch(newEpoch);
    const { panel } = court.drawJury(disputeId);
    assert.equal(panel.length, 3);
  });

  test("reschedule before the locked epoch passes is rejected", () => {
    const court = makeCourt(3);
    const disputeId = openFunded(court);
    assert.throws(() => court.rescheduleDraw(disputeId), /draw epoch has not passed yet/);
  });

  test("draw before the locked epoch is rejected, and drawing twice is rejected", () => {
    const court = makeCourt(3);
    const disputeId = openFunded(court);
    court.setEpoch(1);
    assert.throws(
      () => court.drawJury(disputeId),
      /draw_jury only executes during the locked draw epoch/,
    );
    court.setEpoch(DRAW_DELAY);
    court.drawJury(disputeId);
    assert.throws(() => court.drawJury(disputeId), /jury already drawn for this dispute/);
  });

  test("reschedule of the appeal draw moves beyond the current epoch", () => {
    const court = makeCourt();
    const { disputeId } = disputeWithRuling(court, [VOTE_PAY, VOTE_PAY, VOTE_REFUND]);
    court.openAppeal(disputeId, "payer", APPEAL_FEE, DRAW_DELAY);
    const dispute = court.dispute(disputeId);
    const oldEpoch = dispute.appealDrawEpoch;
    court.setEpoch(oldEpoch + 1);
    const newEpoch = court.rescheduleAppealDraw(disputeId);
    assert.equal(newEpoch, oldEpoch + 2);
    court.setEpoch(newEpoch);
    const { panel } = court.drawAppealJury(disputeId);
    assert.equal(panel.length, 7);
  });
});

describe("appeal", () => {
  test("appeal overturns the first ruling: refund four to three, payer net minus 30,000", () => {
    const court = makeCourt();
    const { disputeId, panel, appealPanel } = disputeWithAppealDrawn(court, [
      VOTE_PAY,
      VOTE_PAY,
      VOTE_REFUND,
    ]);
    const payerBefore = court.balance("payer");
    const payeeBefore = court.balance("payee");
    const appealVotes = [VOTE_PAY, VOTE_PAY, VOTE_PAY, VOTE_PAY, VOTE_PAY, VOTE_PAY, VOTE_PAY];
    let refunds = 0;
    appealPanel.forEach((key, i) => {
      if (refunds < 4 && !panel.slice(0, 2).includes(key)) {
        appealVotes[i] = VOTE_REFUND;
        refunds += 1;
      }
    });
    assert.equal(refunds, 4);
    ruleAppealPanel(court, disputeId, appealPanel, appealVotes);
    const result = settleAtDeadline(court, disputeId);
    assert.equal(result.outcome, VOTE_REFUND);
    assert.equal(result.payeeAmount, 0n);
    assert.equal(court.balance("payee"), payeeBefore);
    assert.equal(court.balance("payer"), payerBefore + 490000n + 15000n);
    assert.equal(court.escrows.get("escrow-0").vault, 0n);
  });

  test("appeal guards reject a bad fee, a stranger, and a double appeal", () => {
    const court = makeCourt();
    const { disputeId } = disputeWithRuling(court, [VOTE_PAY, VOTE_PAY, VOTE_REFUND]);
    assert.throws(
      () => court.openAppeal(disputeId, "payer", APPEAL_FEE - 1, DRAW_DELAY),
      /appeal fee must be exactly APPEAL_FEE/,
    );
    assert.throws(
      () => court.openAppeal(disputeId, "stranger", APPEAL_FEE, DRAW_DELAY),
      /only a party to the dispute can appeal/,
    );
    court.openAppeal(disputeId, "payer", APPEAL_FEE, DRAW_DELAY);
    assert.throws(
      () => court.openAppeal(disputeId, "payer", APPEAL_FEE, DRAW_DELAY),
      /appeal already opened/,
    );
  });

  test("appeal after settlement is rejected", () => {
    const court = makeCourt();
    const { disputeId } = disputeWithRuling(court, [VOTE_PAY, VOTE_PAY, VOTE_REFUND]);
    settleAtDeadline(court, disputeId);
    assert.throws(
      () => court.openAppeal(disputeId, "payer", APPEAL_FEE, DRAW_DELAY),
      /dispute already settled/,
    );
  });

  test("appeal replacement is rejected before the deadline and for a stranger", () => {
    const court = makeCourt();
    const { disputeId, appealPanel } = disputeWithAppealDrawn(court, [
      VOTE_PAY,
      VOTE_PAY,
      VOTE_REFUND,
    ]);
    ruleAppealPanel(court, disputeId, appealPanel.slice(0, 3), [
      VOTE_REFUND,
      VOTE_REFUND,
      VOTE_REFUND,
    ]);
    assert.throws(
      () => court.replaceAppealJury(disputeId, "payer", DRAW_DELAY),
      /appeal reveal window has not passed yet/,
    );
    const dispute = court.dispute(disputeId);
    court.setEpoch(dispute.appealRevealDeadline);
    assert.throws(
      () => court.replaceAppealJury(disputeId, "stranger", DRAW_DELAY),
      /only a party to the dispute can replace the appeal jury/,
    );
  });

  test("appeal replacement recovers after a no-show", () => {
    const court = makeCourt();
    const { disputeId, appealPanel } = disputeWithAppealDrawn(court, [
      VOTE_PAY,
      VOTE_PAY,
      VOTE_REFUND,
    ]);
    ruleAppealPanel(court, disputeId, appealPanel.slice(0, 6), [
      VOTE_REFUND,
      VOTE_REFUND,
      VOTE_REFUND,
      VOTE_PAY,
      VOTE_PAY,
      VOTE_PAY,
    ]);
    const dispute = court.dispute(disputeId);
    court.setEpoch(dispute.appealRevealDeadline);
    const newEpoch = court.replaceAppealJury(disputeId, "payer", DRAW_DELAY);
    assert.equal(newEpoch, dispute.appealRevealDeadline + DRAW_DELAY);
    court.setEpoch(newEpoch);
    const redrawn = court.drawAppealJury(disputeId);
    assert.equal(redrawn.panel.length, 7);
    ruleAppealPanel(court, disputeId, redrawn.panel, [
      VOTE_REFUND,
      VOTE_REFUND,
      VOTE_REFUND,
      VOTE_REFUND,
      VOTE_PAY,
      VOTE_PAY,
      VOTE_PAY,
    ]);
    const result = settleAtDeadline(court, disputeId);
    assert.equal(result.outcome, VOTE_REFUND);
    assert.equal(result.payeeAmount, 0n);
    assert.equal(court.escrows.get("escrow-0").vault, 0n);
  });

  test("appeal with fewer than seven jurors is rejected", () => {
    const court = makeCourt(3);
    const { disputeId } = disputeWithRuling(court, [VOTE_PAY, VOTE_PAY, VOTE_REFUND]);
    assert.throws(
      () => court.openAppeal(disputeId, "payer", APPEAL_FEE, DRAW_DELAY),
      /not enough eligible jurors to draw an appeal panel/,
    );
  });
});

describe("pool minimums", () => {
  test("opening a dispute with two jurors is rejected", () => {
    const court = makeCourt(2);
    court.fund("payer", 1000000);
    court.createEscrow("escrow-0", "payer", "payee", TOTAL);
    assert.throws(
      () => court.openDispute("escrow-0", DRAW_DELAY),
      /not enough eligible jurors to draw a panel/,
    );
  });

  test("registering a juror below the bond floor is rejected, and twice is rejected", () => {
    const court = new SimCourt();
    court.fund("new-juror", 100000);
    assert.throws(() => court.registerJuror("new-juror", BOND_FLOOR - 1), /bond below the floor/);
    court.registerJuror("new-juror", BOND_FLOOR);
    assert.throws(
      () => court.registerJuror("new-juror", BOND_FLOOR),
      /juror already registered/,
    );
  });
});

describe("evidence", () => {
  test("a stranger cannot submit evidence", () => {
    const court = makeCourt(3);
    const disputeId = openFunded(court);
    assert.throws(
      () => court.submitEvidence(disputeId, "stranger", 0, 0, sha256Hex("a"), "sample://a"),
      /only a party to the dispute can submit evidence/,
    );
  });

  test("evidence is limited to three per side and immutable once posted", () => {
    const court = makeCourt(3);
    const disputeId = openFunded(court);
    for (let i = 0; i < 3; i += 1) {
      court.submitEvidence(disputeId, "payer", 0, i, sha256Hex(`evidence-${i}`), `sample://${i}`);
    }
    assert.throws(
      () => court.submitEvidence(disputeId, "payer", 0, 3, sha256Hex("x"), "sample://x"),
      /evidence index out of range/,
    );
    assert.throws(
      () => court.submitEvidence(disputeId, "payer", 0, 0, sha256Hex("changed"), "sample://y"),
      /evidence already posted at this index/,
    );
    assert.equal(court.dispute(disputeId).payerEvidence[0].sha256, sha256Hex("evidence-0"));
  });
});

describe("commit and reveal", () => {
  test("a reveal with the wrong salt is rejected", () => {
    const court = makeCourt(3);
    const disputeId = openFunded(court);
    const { panel } = drawAt(court, disputeId);
    court.commitVote(disputeId, panel[0], VOTE_PAY, salt(0));
    assert.throws(
      () => court.revealVote(disputeId, panel[0], VOTE_PAY, salt(1)),
      /reveal does not match the commitment/,
    );
  });

  test("a double commit and a double reveal are rejected", () => {
    const court = makeCourt(3);
    const disputeId = openFunded(court);
    const { panel } = drawAt(court, disputeId);
    court.commitVote(disputeId, panel[0], VOTE_PAY, salt(0));
    assert.throws(
      () => court.commitVote(disputeId, panel[0], VOTE_PAY, salt(0)),
      /juror already committed a vote/,
    );
    court.revealVote(disputeId, panel[0], VOTE_PAY, salt(0));
    assert.throws(
      () => court.revealVote(disputeId, panel[0], VOTE_PAY, salt(0)),
      /juror already revealed a vote/,
    );
  });

  test("a juror outside the panel cannot commit", () => {
    const court = makeCourt(3);
    const disputeId = openFunded(court);
    drawAt(court, disputeId);
    const outsider = sha256Hex("sim-juror-9");
    court.fund(outsider, 100000);
    court.registerJuror(outsider, BOND_FLOOR);
    assert.throws(
      () => court.commitVote(disputeId, outsider, VOTE_PAY, salt(0)),
      /juror not drawn for this dispute/,
    );
  });

  test("a reveal without a commitment is rejected", () => {
    const court = makeCourt(3);
    const disputeId = openFunded(court);
    const { panel } = drawAt(court, disputeId);
    assert.throws(
      () => court.revealVote(disputeId, panel[0], VOTE_PAY, salt(0)),
      /no vote commitment from this juror/,
    );
  });
});

describe("escrow rules", () => {
  test("a ruling executes at most once and the remainder only after a ruling", () => {
    const escrow = new SimEscrow("escrow-1", "payer", "payee", TOTAL);
    escrow.freezeForDispute(0);
    assert.throws(() => escrow.withdrawRemainder(), /no ruling has executed/);
    assert.equal(escrow.executeRuling(0, 490000n), 490000n);
    assert.throws(() => escrow.executeRuling(0, 490000n), /ruling already executed/);
    assert.equal(escrow.withdrawRemainder(), 10000n);
    assert.equal(escrow.vault, 0n);
  });

  test("an unfunded escrow cannot freeze for a dispute", () => {
    const escrow = new SimEscrow("escrow-2", "payer", "payee", 0);
    assert.throws(() => escrow.freezeForDispute(0), /escrow vault is empty/);
  });
});

describe("sample dispute 0 reproduced by the simulation", () => {
  test("the same votes rule the same outcome with the same amounts", () => {
    const sample = SAMPLE.disputes[0];
    const court = new SimCourt();
    for (const juror of SAMPLE.jurors) {
      court.fund(juror.key, 100000);
      court.registerJuror(juror.key, BOND_FLOOR);
    }
    court.fund("payer", 1000000);
    court.createEscrow("escrow-0", "payer", "payee", sample.principal);
    const disputeId = court.openDispute("escrow-0", DRAW_DELAY);
    court.setEpoch(DRAW_DELAY);
    const { panel } = court.drawJury(disputeId);
    rulePanel(court, disputeId, panel, [VOTE_PAY, VOTE_PAY, VOTE_REFUND]);
    const result = settleAtDeadline(court, disputeId);
    assert.equal(result.outcome, VOTE_PAY);
    assert.equal(result.payeeAmount, BigInt(sample.payeeAmount));
    assert.equal(result.payerAmount, BigInt(sample.payerAmount));
    assert.equal(result.fees, BigInt(sample.fees));
    assert.equal(
      result.payeeAmount + result.payerAmount + result.fees,
      BigInt(sample.principal),
    );
  });
});

describe("input validation", () => {
  test("a negative evidence index is rejected", () => {
    const court = makeCourt(3);
    const disputeId = openFunded(court);
    assert.throws(
      () => court.submitEvidence(disputeId, "payer", 0, -1, sha256Hex("a"), "sample://a"),
      /evidence index out of range/,
    );
    assert.throws(
      () => court.submitEvidence(disputeId, "payer", 0, 1.5, sha256Hex("a"), "sample://a"),
      /evidence index out of range/,
    );
  });

  test("negative, fractional, and NaN votes are rejected on commit and reveal", () => {
    const court = makeCourt(3);
    const disputeId = openFunded(court);
    const { panel } = drawAt(court, disputeId);
    for (const bad of [-1, 1.5, Number.NaN, 4]) {
      assert.throws(
        () => court.commitVote(disputeId, panel[0], bad, salt(0)),
        /invalid vote/,
      );
    }
    court.commitVote(disputeId, panel[0], VOTE_PAY, salt(0));
    for (const bad of [-1, 1.5, Number.NaN, 4]) {
      assert.throws(
        () => court.revealVote(disputeId, panel[0], bad, salt(0)),
        /invalid vote/,
      );
    }
  });

  test("a duplicate escrow id and an invalid principal are rejected", () => {
    const court = makeCourt(3);
    court.fund("payer", 1000000);
    court.createEscrow("escrow-0", "payer", "payee", TOTAL);
    assert.throws(
      () => court.createEscrow("escrow-0", "payer", "payee", TOTAL),
      /escrow already exists/,
    );
    for (const [i, bad] of [0, -5, 1.5, Number.NaN].entries()) {
      assert.throws(
        () => court.createEscrow(`escrow-bad-${i}`, "payer", "payee", bad),
        /principal must be more than 0|amount must be a whole number/,
      );
    }
    assert.throws(
      () => court.createEscrow("escrow-9", "payer", "payer", TOTAL),
      /payee must differ from the payer/,
    );
  });
});

describe("settlement precision", () => {
  test("a principal above the safe integer range settles to the micro-Tari", () => {
    const principal = 9007199254740993n;
    const court = makeCourt(3);
    court.fund("payer", principal);
    court.createEscrow("escrow-0", "payer", "payee", principal);
    const disputeId = court.openDispute("escrow-0", DRAW_DELAY);
    const { panel } = drawAt(court, disputeId);
    rulePanel(court, disputeId, panel, [VOTE_PAY, VOTE_PAY, VOTE_REFUND]);
    const result = settleAtDeadline(court, disputeId);
    assert.equal(result.fees, 10000n);
    assert.equal(result.payeeAmount, principal - 10000n);
    assert.equal(result.payerAmount, 0n);
    assert.equal(
      result.payeeAmount + result.payerAmount + result.fees,
      principal,
    );
    assert.equal(court.balance("payee"), principal - 10000n);
  });
});

describe("reveal deadline gate on settle", () => {
  test("settle before the reveal deadline is rejected, at the deadline it runs", () => {
    const court = makeCourt(3);
    const { disputeId } = disputeWithRuling(court, [VOTE_PAY, VOTE_PAY, VOTE_REFUND]);
    assert.throws(() => court.settle(disputeId), /reveal window has not passed yet/);
    const result = settleAtDeadline(court, disputeId);
    assert.equal(result.outcome, VOTE_PAY);
    assert.equal(result.payeeAmount, 490000n);
  });
});

describe("close after a replacement", () => {
  test("close is rejected until the replacement round finishes", () => {
    const court = makeCourt(3);
    const disputeId = openFunded(court);
    const { panel } = drawAt(court, disputeId);
    rulePanel(court, disputeId, panel.slice(0, 1), [VOTE_PAY]);
    const dispute = court.dispute(disputeId);
    court.setEpoch(dispute.revealDeadline);
    court.replaceJury(disputeId, "payer", DRAW_DELAY);
    assert.throws(
      () => court.closeNoRuling(disputeId),
      /jury not drawn for this dispute/,
    );
    court.setEpoch(court.dispute(disputeId).drawEpoch);
    const redrawn = court.drawJury(disputeId);
    rulePanel(court, disputeId, redrawn.panel.slice(0, 1), [VOTE_REFUND]);
    assert.throws(
      () => court.closeNoRuling(disputeId),
      /first round can still reach a ruling/,
    );
    court.setEpoch(court.dispute(disputeId).revealDeadline);
    const result = court.closeNoRuling(disputeId);
    assert.equal(result.payerAmount, BigInt(TOTAL));
    assert.equal(court.dispute(disputeId).closedNoRuling, true);
  });
});

describe("refusing juror in the no-ruling close", () => {
  test("a refusing juror is neither paid nor slashed", () => {
    const court = makeCourt(3);
    const disputeId = openFunded(court);
    const { panel } = drawAt(court, disputeId);
    rulePanel(court, disputeId, panel.slice(0, 2), [VOTE_REFUSE, VOTE_PAY]);
    const failedFirst = panel[2];
    const dispute = court.dispute(disputeId);
    court.setEpoch(dispute.revealDeadline);
    court.replaceJury(disputeId, "payer", DRAW_DELAY);
    court.setEpoch(court.dispute(disputeId).drawEpoch);
    const redrawn = court.drawJury(disputeId);
    rulePanel(court, disputeId, redrawn.panel.slice(0, 2), [VOTE_REFUSE, VOTE_PAY]);
    court.setEpoch(court.dispute(disputeId).revealDeadline);
    court.closeNoRuling(disputeId);
    const refuseKey = redrawn.panel[0];
    const payKey = redrawn.panel[1];
    const silentKey = redrawn.panel[2];
    assert.equal(court.disputeClaim(disputeId, refuseKey), 0n);
    const expectedPay = silentKey === failedFirst ? 5000n : 10000n;
    assert.equal(court.disputeClaim(disputeId, payKey), expectedPay);
    const expectedRefuseBond =
      refuseKey === failedFirst ? BigInt(BOND_FLOOR - JUROR_FEE) : BigInt(BOND_FLOOR);
    assert.equal(court.bonds.get(refuseKey), expectedRefuseBond);
    assert.equal(court.bonds.get(failedFirst), BigInt(BOND_FLOOR - JUROR_FEE));
    assert.equal(court.bonds.get(silentKey), BigInt(BOND_FLOOR - JUROR_FEE));
  });
});

describe("failed jurors stay in their own round", () => {
  test("appeal failed jurors top up the appeal majority only", () => {
    const court = makeCourt();
    const { disputeId, panel, appealPanel } = disputeWithAppealDrawn(court, [
      VOTE_PAY,
      VOTE_PAY,
      VOTE_REFUND,
    ]);
    ruleAppealPanel(court, disputeId, appealPanel.slice(0, 6), [
      VOTE_REFUND,
      VOTE_REFUND,
      VOTE_REFUND,
      VOTE_PAY,
      VOTE_PAY,
      VOTE_PAY,
    ]);
    const failedAppeal = appealPanel[6];
    const dispute = court.dispute(disputeId);
    court.setEpoch(dispute.appealRevealDeadline);
    court.replaceAppealJury(disputeId, "payer", DRAW_DELAY);
    court.setEpoch(court.dispute(disputeId).appealDrawEpoch);
    const redrawn = court.drawAppealJury(disputeId);
    const appealVotes = [
      VOTE_REFUND,
      VOTE_REFUND,
      VOTE_REFUND,
      VOTE_REFUND,
      VOTE_PAY,
      VOTE_PAY,
      VOTE_PAY,
    ];
    ruleAppealPanel(court, disputeId, redrawn.panel, appealVotes);
    const result = settleAtDeadline(court, disputeId);
    assert.equal(result.outcome, VOTE_REFUND);
    // The appeal slash is one fee per juror in the minority plus the one
    // failed juror, counted once even when the failed juror also voted
    // with the minority in the second round.
    const appealSlashKeys = new Set(
      redrawn.panel.filter((key, i) => appealVotes[i] === VOTE_PAY),
    );
    appealSlashKeys.add(failedAppeal);
    const appealShareEach = 5000n + (BigInt(appealSlashKeys.size * 5000) / 4n);
    for (const key of court.jurors.keys()) {
      const firstShare = panel.slice(0, 2).includes(key) ? 7500n : 0n;
      const appealIndex = redrawn.panel.indexOf(key);
      const appealShare = appealVotes[appealIndex] === VOTE_REFUND ? appealShareEach : 0n;
      assert.equal(court.disputeClaim(disputeId, key), firstShare + appealShare);
    }
    assert.equal(court.bonds.get(failedAppeal), BigInt(BOND_FLOOR - JUROR_FEE));
  });
});

describe("reschedule cap", () => {
  test("a draw can be rescheduled three times, the fourth is refused", () => {
    const court = makeCourt(3);
    const disputeId = openFunded(court);
    let drawEpoch = court.dispute(disputeId).drawEpoch;
    for (let expected = 1; expected <= 3; expected += 1) {
      court.setEpoch(drawEpoch + 1);
      const next = court.rescheduleDraw(disputeId);
      assert.equal(court.dispute(disputeId).reschedules, expected);
      drawEpoch = next;
    }
    court.setEpoch(drawEpoch + 1);
    assert.throws(() => court.rescheduleDraw(disputeId), /draw reschedule limit reached/);
  });

  test("an appeal draw can be rescheduled three times, the fourth is refused", () => {
    const court = makeCourt();
    const { disputeId } = disputeWithRuling(court, [VOTE_PAY, VOTE_PAY, VOTE_REFUND]);
    court.openAppeal(disputeId, "payer", APPEAL_FEE, DRAW_DELAY);
    let drawEpoch = court.dispute(disputeId).appealDrawEpoch;
    for (let expected = 1; expected <= 3; expected += 1) {
      court.setEpoch(drawEpoch + 1);
      const next = court.rescheduleAppealDraw(disputeId);
      assert.equal(court.dispute(disputeId).appealReschedules, expected);
      drawEpoch = next;
    }
    court.setEpoch(drawEpoch + 1);
    assert.throws(
      () => court.rescheduleAppealDraw(disputeId),
      /appeal draw reschedule limit reached/,
    );
  });
});
