// Generates web/src/sample-data.js from the template rules in
// web/src/verdict.js, with every hash computed here by node:crypto, so the
// sample disputes the proof page shows are internally consistent: the panel
// is really derived from the draw hash, and every commitment is really the
// SHA-256 of (vote, salt, dispute id, juror). Run: node tools/generate-sample-data.mjs
//
// Every value in the output is sample data, labelled as such on the page.
// No key here is a real person, no hash is on chain, no amount moved.

import { createHash } from "node:crypto";
import { writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import {
  derivePanel,
  voteCommitmentBytes,
  bytesToHex,
  VOTE_PAY,
  VOTE_REFUND,
} from "../src/verdict.js";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");

function sha256Hex(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

function sha256Text(text) {
  return createHash("sha256").update(text, "utf8").digest("hex");
}

// Sample jurors: keys are SHA-256 of a label, so they are deterministic and
// plainly not real keys. The demo pool is seeded with known testers and the
// pool is disclosed on the Become a juror page.
const jurors = Array.from({ length: 7 }, (_, i) => ({
  index: i,
  label: `Tester juror ${i + 1}`,
  key: sha256Text(`verdict-sample-juror-${i}`),
  bond: 30000,
}));

const pool = jurors.map((j) => j.key);

function evidenceItem(side, index, text) {
  const bytes = Buffer.from(text, "utf8");
  return {
    side,
    index,
    uri: `sample://evidence/${side}-${index}.txt`,
    text,
    bytes_len: bytes.length,
    sha256: sha256Hex(bytes),
  };
}

function buildDispute(config) {
  const drawHash = sha256Text(`verdict-sample-draw-epoch-${config.drawEpoch}`);
  const panelIndexes = derivePanel(pool, drawHash, config.id, 0);
  const panel = panelIndexes.map((i) => jurors[i]);
  const commits = config.votes.map((vote, k) => {
    const juror = panel[k];
    const salt = sha256Text(`verdict-sample-salt-${config.id}-${k}`);
    const commitment = sha256Hex(voteCommitmentBytes(vote, salt, config.id, juror.key));
    return { juror: juror.key, jurorLabel: juror.label, vote, salt, commitment };
  });
  return {
    ...config,
    drawHash,
    panel: panel.map((j) => ({ key: j.key, label: j.label })),
    panelIndexes,
    commits,
    reveals: commits.map((c) => ({ juror: c.juror, jurorLabel: c.jurorLabel, vote: c.vote })),
    sample: true,
  };
}

const disputes = [
  buildDispute({
    id: 0,
    title: "Milestone 1 delivery disputed after the client went silent",
    escrowType: "Milestone escrow",
    escrow: "sample-component-milestone-0",
    payer: "Ada, client",
    payerKey: sha256Text("verdict-sample-payer-ada"),
    payee: "Bola, freelancer",
    payeeKey: sha256Text("verdict-sample-payee-bola"),
    principal: 500000,
    milestones: [200000, 300000],
    openEpoch: 13370,
    drawEpoch: 13372,
    round: 0,
    status: "settled",
    votes: [VOTE_PAY, VOTE_PAY, VOTE_REFUND],
    outcome: "pay",
    payeeAmount: 500000,
    ruling: "Pay. Two of three jurors ruled the milestone was delivered, so the full milestone amount goes to the payee.",
    settlementTx: "sample-tx-settle-dispute-0",
    evidence: [
      evidenceItem("payer", 0, "Client note: no reply to the delivery message after 9 days."),
      evidenceItem("payee", 0, "Delivered: milestone 1 code, demo video, and readme, 12 Nov 2026."),
      evidenceItem("payee", 1, "Delivery receipt: demo link opened by the client on 13 Nov 2026."),
    ],
  }),
  buildDispute({
    id: 1,
    title: "Single payment for a logo, work never started",
    escrowType: "Payment escrow",
    escrow: "sample-component-payment-1",
    payer: "Ada, client",
    payerKey: sha256Text("verdict-sample-payer-ada"),
    payee: "Chidi, designer",
    payeeKey: sha256Text("verdict-sample-payee-chidi"),
    principal: 300000,
    milestones: [300000],
    openEpoch: 13371,
    drawEpoch: 13373,
    round: 0,
    status: "settled",
    votes: [VOTE_REFUND, VOTE_REFUND, VOTE_PAY],
    outcome: "refund",
    payeeAmount: 0,
    ruling: "Refund. Two of three jurors ruled the work never started, so the amount returns to the payer.",
    settlementTx: "sample-tx-settle-dispute-1",
    evidence: [
      evidenceItem("payer", 0, "Payment receipt: 300,000 micro-Tari locked in escrow."),
      evidenceItem("payee", 0, "Designer note: start date moved twice, no files delivered."),
    ],
  }),
  buildDispute({
    id: 2,
    title: "Milestone 2 scope disputed, voting open",
    escrowType: "Milestone escrow",
    escrow: "sample-component-milestone-2",
    payer: "Ada, client",
    payerKey: sha256Text("verdict-sample-payer-ada"),
    payee: "Bola, freelancer",
    payeeKey: sha256Text("verdict-sample-payee-bola"),
    principal: 500000,
    milestones: [200000, 300000],
    openEpoch: 13374,
    drawEpoch: 13376,
    round: 0,
    status: "voting open",
    votes: [],
    outcome: null,
    payeeAmount: null,
    ruling: null,
    settlementTx: null,
    evidence: [
      evidenceItem("payer", 0, "Client note: milestone 2 delivered work does not match the agreed scope."),
      evidenceItem("payee", 0, "Delivered: milestone 2 code and scope note, 20 Nov 2026."),
    ],
  }),
];

// Dispute 2 has no votes yet, so drop its empty commits and reveals.
disputes[2].commits = [];
disputes[2].reveals = [];

const data = { jurors, pool, disputes };

const out = `// Sample data for the Verdict web app. Generated by
// tools/generate-sample-data.mjs. Every hash in here was computed from the
// bytes it claims to cover, so the proof page checks pass on the sample
// inputs. Nothing in here is live chain data, no key is a real person, and
// no amount moved. Regenerate after any rule change, do not edit by hand.

export const SAMPLE = ${JSON.stringify(data, null, 2)};
`;

writeFileSync(join(root, "src", "sample-data.js"), out);
console.log(`wrote src/sample-data.js: ${disputes.length} sample disputes, ${jurors.length} sample jurors`);
