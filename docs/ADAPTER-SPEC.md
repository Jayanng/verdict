# Adapter spec

The interface any escrow implements to ship with the Verdict court built in.
The escrow keeps custody of its own vault at all times. The court issues
rulings; the escrow's own code executes them.

## The two-method interface

```rust
freeze_for_dispute(dispute_id)
execute_ruling(dispute_id, outcome, amounts)
```

- `freeze_for_dispute(dispute_id)`: called by the arbitrator component when
  a dispute opens. Freezes the disputed vault so the milestone cannot move
  while the court rules.
- `execute_ruling(dispute_id, outcome, amounts)`: called only by the
  arbitrator component when the court settles. Moves the escrow's own vault
  exactly once, per the ruling, and enforces microTari conservation in the
  template code. The outcome is one of pay, refund, or split, as defined in
  [SETTLEMENT.md](SETTLEMENT.md).

## Constructor

The escrow constructor takes `arbitrator: ComponentAddress`, the address of
the Verdict court component for the deal. The address is stored immutable:
it is set at creation and can never be changed afterwards. A deal names its
court at creation, and no one can swap in a different court later.

## Gating

Three layers, all required:

1. **Access rule**: the settlement methods accept calls only from
   `rule!(caller_component(arbitrator))`. Only calls from the court component
   named at creation are accepted.
2. **In-method assert**: each method asserts `caller_component ==
   arbitrator` before doing anything, as defense in depth against any
   access-rule subtlety.
3. **Ownerless escrow**: the escrow is created with `OwnerRule::None`. There
   is no owner, no admin method, and no upgrade path, so no owner-bypass
   path exists that could rewrite the rules or drain the vault.

## Reference implementation

PieceWork is the reference implementation: a milestone escrow for freelance
work, forked at https://github.com/Jayanng/piecework and adapted in
`templates/arbitratable-escrow`. It keeps `OwnerRule::None` from the
original, adds the two methods above, and takes the arbitrator address in
its constructor.
