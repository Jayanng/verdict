# Verdict

**The Ootle jury court.**

When an Ootle escrow deal goes wrong, three bonded strangers rule it and the loser pays, the court never touches the money, the ownerless escrow executes the ruling, and any escrow can ship with the court built in.

Verdict is an entry in the Tari November Build Contest, theme MVP.

## What ships

| Artifact | What it is |
|---|---|
| `templates/verdict` | The ownerless court: juror pool (XTM bond), disputes, evidence hashes, future-epoch jury draw, commit-reveal voting, settlement, appeal, and fee ledger. |
| `templates/milestone-escrow` | A milestone escrow that ships with the court built in: the client funds milestone amounts, the freelancer marks each delivered, the client approves, and approval releases exactly that milestone's slice. |
| `templates/payment-escrow` | A single-payment escrow that ships with the court built in: one amount, one delivery, the payer approves to release, the deadline refunds. |
| `web/` | Landing page and app: walletless simulation and the live path via Sapient, with dispute proof pages. |

## How it works

The court never touches the money. It issues a ruling; the escrow's own ownerless code executes it, and its settlement method accepts calls from exactly one component: the court.

An escrow adopts Verdict by implementing the two-method interface in [docs/ADAPTER-SPEC.md](docs/ADAPTER-SPEC.md) and naming the court at creation.

## Docs

- [docs/ADAPTER-SPEC.md](docs/ADAPTER-SPEC.md): the interface any escrow implements to ship with a court.
- [docs/SETTLEMENT.md](docs/SETTLEMENT.md): how rulings settle, and the rule that every settlement path conserves microTari exactly.

## License

MIT. See [LICENSE](LICENSE).
