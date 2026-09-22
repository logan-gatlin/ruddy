#!/usr/bin/env python3
"""Exhaustively compare extend4's inferred and factored presence formulas.

The baseline is the inferred signature of ../img/src/main.rud's extend4,
captured on 2026-09-21. There are four input-slot presences, four input
#None-case presences, and four output #None-case presences. The formula
below embeds all 25 printed clauses, grouping repetitive clauses by index.
This checks Boolean equivalence only; it does not check type payloads,
row-tail lacks constraints, annotations, or proposed new syntax.
"""

from itertools import product


def inferred_formula(slots, input_none, output_none):
    """One prefix chain and 24 remaining clauses in the current signature."""
    return (
        all(not slots[i + 1] or slots[i] for i in range(3))
        # Ten clauses: a missing slot forces this and all later outputs None.
        and all(slots[i] or output_none[j] for i in range(4) for j in range(i, 4))
        # Four clauses: copied slots cannot gain an admitted None case.
        and all(
            not any(slots[k] and output_none[i] for k in range(i, 4))
            or input_none[i]
            for i in range(4)
        )
        # Four clauses: every admitted input None case survives.
        and all(not input_none[i] or output_none[i] for i in range(4))
        # Six clauses relating output None cases to later positions.
        and all(
            (not output_none[i] or output_none[j]) or input_none[i]
            for i in range(4)
            for j in range(i + 1, 4)
        )
    )


def factored_formula(slots, input_none, output_none):
    """One prefix chain plus four output_none = not slot or input_none equations."""
    return all(not slots[i + 1] or slots[i] for i in range(3)) and all(
        output_none[i] == (not slots[i] or input_none[i]) for i in range(4)
    )


def main():
    admitted = 0
    checked = 0
    for bits in product((False, True), repeat=12):
        slots, input_none, output_none = bits[:4], bits[4:8], bits[8:]
        original = inferred_formula(slots, input_none, output_none)
        factored = factored_formula(slots, input_none, output_none)
        assert original == factored, (slots, input_none, output_none)
        checked += 1
        admitted += original
    print(
        f"Equivalent over all {checked} Boolean assignments "
        f"({admitted} satisfy both formulas)."
    )
    print("25 printed clauses reduce to one prefix chain plus four equations.")
    print("Keep the original type body, payloads, and row tails unchanged.")


if __name__ == "__main__":
    main()
