First reference[^b] establishes footnote b as number one.

Footnote a[^a] is defined first in source order, but is referenced second, so it becomes number two.

A second reference to footnote b[^b] shares its number but gets its own backreference id.

An empty-looking label[^!!!] falls back to a numeric id.

Duplicate definitions collapse: the first one wins.

[^a]: Definition for a, defined first in source order.

[^b]: Definition for b, but referenced first in the document.

[^!!!]: A label that slugifies to nothing.

[^b]: A duplicate definition for b that must be discarded.
