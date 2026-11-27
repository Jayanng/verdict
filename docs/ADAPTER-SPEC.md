# How to add a court to any Ootle escrow

Any Ootle escrow can ship with the Verdict court built in. The escrow keeps
custody of its own vault at all times. The court issues rulings; the escrow's
own code executes them. This guide is the adapter spec: the interface, the
constructor, the gating, and the rules an escrow must follow to adopt it.

## The two-method interface

```rust
freeze_for_dispute(dispute_id)
execute_ruling(dispute_id, outcome, amounts)
```

- `freeze_for_dispute(dispute_id)`: called by the arbitrator component when
  a dispute opens. Freezes the disputed vault so no party can move the
  money while the court rules. Freezing blocks every state change on the
  escrow, including a refund that would drain the vault while the court is
  ruling. A second freeze while frozen is refused.
- `execute_ruling(dispute_id, outcome, amounts)`: called only by the
  arbitrator component when the court settles, once. `outcome` is the
  ruling outcome, with the court's vote values: pay 0, refund 1, split 2,
  no ruling 4. `amounts` is the pair (payee amount, payer amount), one
  argument. The payer amount is gross: the amount routed to the payer
  side before the court takes juror fees out of it into the fee vault.
  The payee amount is net. The call moves the escrow's own vault exactly
  once, per the ruling, and returns both buckets in one atomic call,
  (payee bucket, payer bucket), so the vault is empty when the settlement
  finishes and no second call is needed. The ruling executes at most once
  for a dispute. See [SETTLEMENT.md](SETTLEMENT.md) for the outcomes and
  the rule that every settlement path conserves microTari exactly.
  The court deposits the payee bucket to the payee account and the payer
  bucket, less the juror fees, to the payer account, both derived by the
  court from the payer and payee keys the escrow holds, never from an
  address a caller supplies.

The escrow validates the ruling with checked arithmetic before moving
anything, and a ruling that fails any check is refused and leaves the
escrow untouched:

- The outcome must be one of 0, 1, 2, or 4.
- The payee amount plus the payer amount must equal the vault balance
  exactly.
- A refund ruling requires the payee amount to be 0.
- A no-ruling ruling requires the payee amount to be 0.
- A pay ruling requires the payer amount to be at most the maximum
  juror fee total (three fees, 15,000), because the payer's gross
  amount on a pay is exactly the fee. A pay that hands the whole
  principal to the payer is refused.
- A split ruling requires both amounts to be positive, the payee
  amount to be at most one micro-Tari above the payer amount, and
  the payer amount to be at most the maximum juror fee total above
  the payee amount, because the payer's gross is the fee plus the
  payer's half. A split that hands everything to one side is refused:
  the bounds alone still allow (0, principal), so the positivity
  check closes that gap. When the fees equal the principal, the
  court rules pay instead of split, so the split never sees an
  empty post-fee principal.

The exact fee is the court's responsibility: the court computes it
from the majority count, and the escrow validates the shape of the
amounts against the fee's bounds, not the fee itself.

## Approval deposits to the payee

When the payer approves a milestone or a payment, the escrow deposits
the released slice straight into the payee's account itself. `approve`
takes no account address: the escrow derives the payee's account from
the payee key stored at creation, the same derivation the court uses
for settlement payouts, and does the deposit. The caller never holds
the released bucket and never names the destination, so a payer cannot
approve a milestone and then misroute the money to themselves while the
milestone is permanently marked released. Cancel and refund still return
a bucket to the payer, because the payer is the rightful recipient
there.

## Freezing needs funds at stake

`freeze_for_dispute` refuses an escrow whose vault is empty. A dispute
over an empty vault would burn the payee's dispute right and waste a jury
draw with no funds ever at stake, so the freeze requires a positive vault
balance.

Both methods take the dispute id where one applies, and
`execute_ruling` refuses a dispute id that does not match the frozen dispute.

## Constructor

The escrow constructor takes `arbitrator: ComponentAddress`, the address of
the Verdict court component for the deal. The address is stored immutable:
it is set at creation and can never be changed afterwards. A deal names its
court at creation, and no one can swap in a different court later.

The constructor also takes the deal itself: for a milestone escrow, the
milestone amounts and the deadline epoch; for a single-payment escrow, the
payment amount and the deadline epoch. The constructor rejects a deal with
no milestones, a non-positive amount, a milestone total that overflows, a payee
who is the payer, and a deadline that is not after the creation epoch.

The production constructor is the only constructor. There are no
verification constructors: the template's `#[template]` macro generates
a dispatcher arm for every public function and does not respect
`#[cfg]`, so a test-only constructor cannot be hidden from a
production build. The escrow's own guards are defense in depth behind
the court, verified by audit, and the end-to-end tests prove
legitimate rulings pass them.

## Gating

Three layers, all required. The pattern is the caller-component rule, the
in-method assert, and the ownerless escrow.

1. **Access rule**: the settlement methods (`freeze_for_dispute`,
   `execute_ruling`) accept calls only from
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
- `execute_ruling` is the settlement path. It withdraws from the vault
  directly and never reuses a payer-only method, so no settlement step
  depends on the transaction signer. A ruling never pays out more than
  the escrow holds, and the one call empties the vault, so no settlement
  leaves funds behind.

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
2. Add `freeze_for_dispute` and `execute_ruling`
   with the signatures above, restricted by the caller-component rule and the
   in-method assert.
3. Create the component with `OwnerRule::None`, with no admin method and
   no upgrade path.
4. Make a frozen dispute block every state change, make the ruling
   execute at most once, and validate the ruling amounts against the
   vault balance before moving anything.
5. Define the deadline semantics in writing, method by method, and test
   each one: refund only after the deadline, every other party method
   only before it.
6. Use checked arithmetic for every total, and check every withdrawal
   against the vault balance.
7. Write engine tests for the adapter: the escrow freezes its own vault
   on `freeze_for_dispute`, moves its own vault exactly once on
   `execute_ruling` and returns both buckets in that one call, rejects a
   stranger calling either method, rejects a second `execute_ruling` on
   the same dispute, rejects a ruling whose amounts do not sum to the
   vault balance, and rejects a refund ruling that pays the payee.
