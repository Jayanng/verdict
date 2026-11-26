// Verdict shared rules, mirrored from templates/verdict/src/lib.rs at 7cba57c.
//
// This module is pure logic with no DOM and no network. The browser app and
// the Node test suite both import it, so the proof page recomputes exactly
// what the template computes. No values here are chain data. Anything the
// page shows as live must come from the indexer, which is not wired yet.

export const PANEL_SIZE = 3;
export const APPEAL_PANEL_SIZE = 7;
export const JUROR_FEE = 5000;
export const BOND_FLOOR = 2 * JUROR_FEE * PANEL_SIZE;
export const APPEAL_FEE = 35000;
export const SLASH = 5000;
export const EVIDENCE_LIMIT = 3;
export const MIN_DRAW_DELAY = 1;
export const REVEAL_WINDOW = 2;
export const DRAW_DOMAIN_FIRST = 0;
export const DRAW_DOMAIN_APPEAL = 1;
// Times a missed draw window can be rescheduled, per draw.
export const MAX_RESCHEDULES = 3;

export const VOTE_PAY = 0;
export const VOTE_REFUND = 1;
export const VOTE_SPLIT = 2;
export const VOTE_REFUSE = 3;

export const VOTE_LABELS = {
  [VOTE_PAY]: "pay",
  [VOTE_REFUND]: "refund",
  [VOTE_SPLIT]: "split",
  [VOTE_REFUSE]: "refuse to arbitrate",
};

export function hexToBytes(hex) {
  if (typeof hex !== "string") throw new Error("hex string must be a string");
  const clean = hex.startsWith("0x") ? hex.slice(2) : hex;
  if (clean.length % 2 !== 0) throw new Error("hex string must have an even length");
  if (!/^[0-9a-fA-F]*$/.test(clean)) {
    throw new Error("hex string must contain only hex characters");
  }
  const out = new Uint8Array(clean.length / 2);
  for (let i = 0; i < out.length; i += 1) {
    out[i] = parseInt(clean.slice(i * 2, i * 2 + 2), 16);
  }
  return out;
}

export function bytesToHex(bytes) {
  return Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
}

export function u64ToLeBytes(value) {
  let v;
  if (typeof value === "bigint") {
    v = value;
  } else if (typeof value === "number" && Number.isInteger(value)) {
    v = BigInt(value);
  } else {
    throw new Error("u64 value must be a whole number");
  }
  if (v < 0n || v > 0xffffffffffffffffn) throw new Error("u64 value out of range");
  const out = new Uint8Array(8);
  for (let i = 0; i < 8; i += 1) {
    out[i] = Number((v >> BigInt(8 * i)) & 0xffn);
  }
  return out;
}

function concatBytes(parts) {
  const total = parts.reduce((sum, part) => sum + part.length, 0);
  const out = new Uint8Array(total);
  let offset = 0;
  for (const part of parts) {
    out.set(part, offset);
    offset += part.length;
  }
  return out;
}

// Pure JavaScript SHA-256, so the proof page can hash bytes in the browser
// with no network and no dependency. Same output as any standard SHA-256.
// Used by the page and checked against node:crypto in the test suite.
const SHA_K = [
  0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
  0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
  0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
  0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
  0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
  0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
  0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
  0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

export function sha256Bytes(data) {
  const bytes = data instanceof Uint8Array ? data : new TextEncoder().encode(data);
  const bitLen = bytes.length * 8;
  const paddedLength = (((bytes.length + 9 + 63) >> 6) << 6);
  const padded = new Uint8Array(paddedLength);
  padded.set(bytes);
  padded[bytes.length] = 0x80;
  const view = new DataView(padded.buffer);
  view.setUint32(paddedLength - 4, bitLen >>> 0, false);
  view.setUint32(paddedLength - 8, Math.floor(bitLen / 0x100000000), false);
  let h0 = 0x6a09e667, h1 = 0xbb67ae85, h2 = 0x3c6ef372, h3 = 0xa54ff53a;
  let h4 = 0x510e527f, h5 = 0x9b05688c, h6 = 0x1f83d9ab, h7 = 0x5be0cd19;
  const w = new Array(64);
  for (let block = 0; block < paddedLength; block += 64) {
    for (let i = 0; i < 16; i += 1) {
      w[i] = view.getUint32(block + i * 4, false);
    }
    for (let i = 16; i < 64; i += 1) {
      const s0 = ((w[i - 15] >>> 7) | (w[i - 15] << 25)) ^ ((w[i - 15] >>> 18) | (w[i - 15] << 14)) ^ (w[i - 15] >>> 3);
      const s1 = ((w[i - 2] >>> 17) | (w[i - 2] << 15)) ^ ((w[i - 2] >>> 19) | (w[i - 2] << 13)) ^ (w[i - 2] >>> 10);
      w[i] = (w[i - 16] + s0 + w[i - 7] + s1) | 0;
    }
    let a = h0, b = h1, c = h2, d = h3, e = h4, f = h5, g = h6, h = h7;
    for (let i = 0; i < 64; i += 1) {
      const s1 = ((e >>> 6) | (e << 26)) ^ ((e >>> 11) | (e << 21)) ^ ((e >>> 25) | (e << 7));
      const ch = (e & f) ^ (~e & g);
      const temp1 = (h + s1 + ch + SHA_K[i] + w[i]) | 0;
      const s0 = ((a >>> 2) | (a << 30)) ^ ((a >>> 13) | (a << 19)) ^ ((a >>> 22) | (a << 10));
      const maj = (a & b) ^ (a & c) ^ (b & c);
      const temp2 = (s0 + maj) | 0;
      h = g; g = f; f = e; e = (d + temp1) | 0; d = c; c = b; b = a; a = (temp1 + temp2) | 0;
    }
    h0 = (h0 + a) | 0; h1 = (h1 + b) | 0; h2 = (h2 + c) | 0; h3 = (h3 + d) | 0;
    h4 = (h4 + e) | 0; h5 = (h5 + f) | 0; h6 = (h6 + g) | 0; h7 = (h7 + h) | 0;
  }
  const out = new Uint8Array(32);
  const outView = new DataView(out.buffer);
  [h0, h1, h2, h3, h4, h5, h6, h7].forEach((v, i) => outView.setUint32(i * 4, v, false));
  return out;
}

export function sha256Hex(data) {
  return bytesToHex(sha256Bytes(data));
}

// The vote commitment, exactly as the template builds it: SHA-256 of
// (vote as little-endian u64, salt, dispute id as little-endian u64, juror
// key), concatenated in that order. This function returns the bytes to hash,
// the caller supplies the SHA-256 step, so the same code runs in the browser
// (WebCrypto) and in Node (node:crypto).
export function voteCommitmentBytes(vote, saltHex, disputeId, jurorHex) {
  return concatBytes([
    u64ToLeBytes(vote),
    hexToBytes(saltHex),
    u64ToLeBytes(disputeId),
    hexToBytes(jurorHex),
  ]);
}

// The panel derivation, exactly as the template derives it. For each slot,
// hash the draw hash together with the domain, the round, the dispute id,
// and the slot, and take the first eight bytes of the digest modulo the
// pool size, probing forward until an unpicked juror is found. The domain
// is a separate hash input, so each round's panel is derived independently
// of the other round's. The derivation makes no promise that two draws
// produce different panels: panels drawn from different domains are
// independent draws, and may still match by chance. Returns the panel as
// pool indices, in order. The pool must already hold a full panel, or the
// draw is refused.
export function derivePanel(pool, drawHashHex, disputeId, round, domain = DRAW_DOMAIN_FIRST, size = PANEL_SIZE) {
  const hashBytes = hexToBytes(drawHashHex);
  if (hashBytes.length !== 32) throw new Error("draw hash must be 32 bytes");
  const poolLen = pool.length;
  if (poolLen < size) throw new Error("not enough eligible jurors");
  const target = size;
  const panel = [];
  for (let slot = 0; slot < target; slot += 1) {
    const input = concatBytes([
      hashBytes,
      u64ToLeBytes(domain),
      u64ToLeBytes(round),
      u64ToLeBytes(disputeId),
      u64ToLeBytes(slot),
    ]);
    const digest = sha256Bytes(input);
    let value = 0n;
    for (let i = 0; i < 8; i += 1) {
      value |= BigInt(digest[i]) << BigInt(8 * i);
    }
    let candidate = Number(value % BigInt(poolLen));
    while (panel.includes(candidate)) {
      candidate = (candidate + 1) % poolLen;
    }
    panel.push(candidate);
  }
  return panel;
}

// Majority of the full panel, exactly as the template rules it. First,
// at least a majority of the panel must have revealed at all, which is the
// quorum. Then one of pay, refund, or split must hold a strict majority of
// the full panel: two matching votes of three for a first round, four
// matching votes of seven for an appeal. Refuse votes never count toward
// an outcome: a panel that only refuses has no ruling. Returns
// { vote, count } or { vote: null, count: 0 } when there is no ruling,
// which is the case the replacement draw and the fallback paths handle.
export function majorityOutcome(reveals, panelSize = PANEL_SIZE) {
  const quorum = Math.floor(panelSize / 2) + 1;
  if (reveals.length < quorum) return { vote: null, count: 0 };
  const counts = new Map();
  for (const vote of reveals) {
    if (vote === VOTE_REFUSE) continue;
    counts.set(vote, (counts.get(vote) || 0) + 1);
  }
  let best = null;
  let bestCount = 0;
  for (const [vote, count] of counts) {
    if (count > bestCount) {
      best = vote;
      bestCount = count;
    }
  }
  if (best !== null && bestCount * 2 > panelSize) {
    return { vote: best, count: bestCount };
  }
  return { vote: null, count: bestCount };
}

// The principal routing for a ruled outcome, exactly as the template
// settles it. The juror fees come first from the principal, capped at the
// principal, and only the post-fee principal is routed: pay sends it to the payee,
// refund returns it to the payer, split divides it by largest remainder,
// with the odd micro-Tari to the payee. Returns { fees, payeeAmount,
// payerAmount, payerAmountNet }. payerAmount is the GROSS payer amount
// the escrow routes in its payer bucket: the whole principal less the
// payee amount. payerAmountNet is what the payer keeps after the court
// takes the juror fees out of that gross payer bucket, so
// payerAmountNet is payerAmount less fees, and payeeAmount plus
// payerAmountNet plus fees equals the principal exactly. The dispute
// record and the sample data store the net payer amount, matching the
// template's dispute_payer_amount. Slashed bonds top up majority juror
// claims on top of the fee, and are paid through claim_fee, not through
// this routing. Amounts are BigInt end to end when the principal is a
// BigInt, so amounts above the safe integer range keep every micro-Tari;
// a Number principal keeps the Number behaviour the page already uses.
function amountToBigInt(value) {
  if (typeof value === "bigint") return value;
  if (typeof value === "number" && Number.isSafeInteger(value) && value >= 0) {
    return BigInt(value);
  }
  throw new Error("amount must be a whole number of micro-Tari");
}

export function settleAmounts(principal, outcome, majorityCount) {
  const asBigInt = typeof principal === "bigint";
  const p = amountToBigInt(principal);
  const count = amountToBigInt(majorityCount);
  const fees = p < BigInt(JUROR_FEE) * count ? p : BigInt(JUROR_FEE) * count;
  const postFee = p - fees;
  const done = (payeeAmount, payerAmountGross) => ({
    fees: asBigInt ? fees : Number(fees),
    payeeAmount: asBigInt ? payeeAmount : Number(payeeAmount),
    payerAmount: asBigInt ? payerAmountGross : Number(payerAmountGross),
    payerAmountNet: asBigInt
      ? payerAmountGross - fees
      : Number(payerAmountGross - fees),
  });
  if (outcome === VOTE_PAY) {
    return done(postFee, fees);
  }
  if (outcome === VOTE_REFUND) {
    return done(0n, p);
  }
  if (outcome === VOTE_SPLIT) {
    const [payeeAmount, payerAmountNet] = splitLargestRemainder(postFee, [1n, 1n]);
    return done(payeeAmount, payerAmountNet + fees);
  }
  throw new Error("settleAmounts needs a ruled outcome: pay, refund, or split");
}

// Split a total across weights by largest remainder, so the parts always sum
// to the total exactly, to the micro-Tari. Ties break toward the earlier part.
// A BigInt total splits in BigInt; a Number total keeps the Number behaviour.
export function splitLargestRemainder(total, weights) {
  if (typeof total === "bigint") {
    const bigWeights = weights.map((w) => amountToBigInt(w));
    const weightSum = bigWeights.reduce((sum, w) => sum + w, 0n);
    if (weightSum <= 0n) throw new Error("weights must sum to more than 0");
    const floors = bigWeights.map((w) => (total * w) / weightSum);
    const remainders = bigWeights.map((w) => (total * w) % weightSum);
    let assigned = floors.reduce((sum, x) => sum + x, 0n);
    const order = remainders
      .map((r, i) => ({ i, r }))
      .sort((a, b) => (a.r > b.r ? -1 : a.r < b.r ? 1 : a.i - b.i));
    const out = [...floors];
    let k = 0;
    while (assigned < total) {
      out[order[k % order.length].i] += 1n;
      assigned += 1n;
      k += 1;
    }
    return out;
  }
  const weightSum = weights.reduce((sum, w) => sum + w, 0);
  if (weightSum <= 0) throw new Error("weights must sum to more than 0");
  const exact = weights.map((w) => (total * w) / weightSum);
  const floors = exact.map((x) => Math.floor(x));
  let assigned = floors.reduce((sum, x) => sum + x, 0);
  const order = exact
    .map((x, i) => ({ i, frac: x - Math.floor(x) }))
    .sort((a, b) => b.frac - a.frac || a.i - b.i);
  const out = [...floors];
  let k = 0;
  while (assigned < total) {
    out[order[k % order.length].i] += 1;
    assigned += 1;
    k += 1;
  }
  return out;
}

// The escrow caller rule, as published: execute_ruling(dispute_id,
// outcome, amounts) and freeze_for_dispute accept calls from exactly one
// component, the court named as arbitrator at creation, and the escrow is
// ownerless, so no owner path can bypass it. One call to execute_ruling
// returns both buckets, (payee bucket, payer bucket), and there is no
// second settlement call. This is a browser-side check of that published
// rule against the published inputs, not a chain call.
export function callerRuleCheck(callerAddress, arbitratorAddress) {
  if (callerAddress === arbitratorAddress) {
    return {
      allowed: true,
      reason: "caller is the court named as arbitrator at creation, so the caller-component rule accepts the call.",
    };
  }
  return {
    allowed: false,
    reason: "caller is not the court named as arbitrator at creation, so the caller-component rule refuses the call, and the in-method assert refuses it too.",
  };
}
