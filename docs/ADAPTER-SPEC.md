# How to add a court to any Ootle escrow

Any Ootle escrow can ship with the Verdict court built in. The escrow keeps
custody of its own vault at all times. The court issues rulings; the escrow's
own code executes them. This guide is the adapter spec: the interface, the
constructor, the gating, and the rules an escrow must follow to adopt it.

## The two-method interface

```rust
freeze_for_dispute(dispute_id)
execute_ruling(dispute_id, payee_amount)
```

- `freeze_for_dispute(dispute_id)`: called by the arbitrator component when
  a dispute opens. Freezes the disputed vault so no party can move the
  money while the court rules. Freezing blocks every state change on the
  escrow, including a refund that would drain the vault while the court is
  ruling. A second freeze while frozen is refused.
- `execute_ruling(dispute_id, payee_amount)`: called only by the arbitrator
  component when the court settles. Moves the escrow's own vault exactly
  once, per the ruling, and returns the payee's share as a bucket. The
  remainder stays in the vault, and the settlement matrix routes it. The
  ruling outcomes map to the payee amount: pay means the payee amount is
  the ruled share, refund means the payee amount is zero, split means the
  payee amount is the payee's ruled share. The payee amount must never
  exceed the vault balance, and the ruling executes at most once for a
  dispute. See [SETTLEMENT.md](SETTLEMENT.md) for the outcomes and the
  rule that every settlement path conserves microTari exactly.

Both methods take the dispute id, and `execute_ruling` refuses a dispute
id that does not match the frozen dispute.

## Constructor

The escrow constructor takes `arbitrator: ComponentAddress`, the address of
the Verdict court component for the deal. The address is stored immutable:
it is set at creation and can never be changed afterwards. A deal names its
court at creation, and no one can swap in a different court later.

The constructor also takes the deal itself: for a milestone escrow, the
milestone amounts and the deadline epoch; for a single-payment escrow, the
payment amount and the deadline epoch. The constructor rejects a deal with
no milestones, a zero amount, a milestone total that overflows, a payee
who is the payer, and a deadline that is not after the creation epoch.

## Gating

Three layers, all required. The pattern is the caller-component rule, the
in-method assert, and the ownerless escrow.

1. **Access rule**: the settlement methods accept calls only from
   `rule!(caller_component(arbitrator))`. Only calls from the court
   component named at creation are accepted. The rule is readable on chain.
2. **In-method assert**: each method asserts the caller is the arbitrator
   before doing anything, as defense in depth against any access-rule
   subtlety. In `tari_template_lib` 0.41 there is no direct caller-address
   getter, so the assert takes the form of a caller-component badge check
   for the arbitrator.
3. **Ownerless escrow**: the escrow is created with `OwnerRule::None`.
   There is no owner, no admin method, and no upgrade path, so no
   owner-bypass path exists that could rewrite the rules or drain the
   vault. The verified public statement is: the only component whose calls
   the escrow's settlement method accepts is the court.

## Deadline and freeze rules

An escrow that adopts the court must define its deadline semantics in
writing, method by method, and follow them:

- Only refund requires the deadline to have passed. Every other party
  method works before the deadline and stops working after it, so the
  deadline is the point where the deal stops moving forward and the
  payer's refund path opens.
- A frozen dispute blocks every state change: funding, delivery, approval,
  cancel, and refund all refuse while frozen.
- A fully released agreement closes. Once every milestone has been
  released, or the single payment has been released, the agreement is
  complete and no further state change is possible. A ruling that has
  executed, a cancel, and a refund are also terminal.
- Cancel and refund handle the empty vault: cancel on an empty vault
  closes the agreement and returns an empty bucket, and refund on an empty
  vault is refused, since there is nothing to return.

## Arithmetic and the settlement path

- Arithmetic is checked everywhere. Totals, funded totals, and released
  totals use checked addition, and every withdrawal is checked against
  the vault balance before it moves.
- `execute_ruling` is the only settlement path. It withdraws from the
  vault directly and never reuses a payer-only method, so no settlement
  step depends on the transaction signer. A ruling never pays out more
  than the escrow holds.

## The two escrows that ship with Verdict

Two escrows implement this interface, with different deal shapes, to prove
the interface is general and fits any deal, not a one-off:

- `templates/milestone-escrow`: a client funds an agreement with milestone
  amounts, the freelancer marks each milestone delivered, the client
  approves, and approval releases exactly that milestone's slice.
  Unreleased money is refundable after the deadline.
- `templates/payment-escrow`: one amount, one delivery, the payer approves
  to release, the deadline refunds.

Both are ownerless, both take the arbitrator in the constructor, and both
implement the same two methods with the same gating. Any escrow that
implements the two methods and names the court at creation adopts the
court the same way, whatever deal shape it carries.

## Adoption checklist

1. Add `arbitrator: ComponentAddress` to the constructor, store it
   immutable, and reject a deal that names no court.
2. Add `freeze_for_dispute` and `execute_ruling` with the signatures
   above, gated by the caller-component rule and the in-method assert.
3. Create the component with `OwnerRule::None`, with no admin method and
   no upgrade path.
4. Make a frozen dispute block every state change, and make the ruling
   execute at most once.
5. Define the deadline semantics in writing, method by method, and test
   each one: refund only after the deadline, every other party method
   only before it.
6. Use checked arithmetic for every total, and check every withdrawal
   against the vault balance.
7. Write engine tests for the adapter: the escrow freezes its own vault
   on `freeze_for_dispute`, moves its own vault exactly once on
   `execute_ruling`, rejects a stranger calling either method, and rejects
   a second `execute_ruling` on the same dispute.
