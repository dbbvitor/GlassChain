# GlassChain Domain

GlassChain coordinates supply-chain offers, inventory automation, and purchase commitments across participating organizations.

## Language

**Supply offer**:
An offer from a seller describing available product, price, currency, quantity, and lead time for a buyer to evaluate.
_Avoid_: inventory offer

**Inventory trigger**:
A rule that watches an inventory condition and may generate a purchase order when the condition is met.
_Avoid_: reorder service

**Approval gate**:
A decision point that must approve an automated purchase before the purchase order is emitted. A denial or failed active gate prevents the automated purchase.
_Avoid_: validation hook

**Purchase order**:
The supply-chain commitment generated when an accepted supply offer or inventory trigger authorizes an automatic purchase.
_Avoid_: order, purchase transaction
