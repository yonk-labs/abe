---
description: Run a multi-model debate on a question and return a synthesized answer with an agreement/disagreement report
---

Use the `abe` debate MCP tool to run a multi-model debate on the question below. Then present, in order:

1. The final synthesized answer.
2. The points of agreement across the models.
3. The points of disagreement.

If the `abe` MCP tool is unavailable, fall back to running `abe debate "$ARGUMENTS"` via the shell and show its output.

Question: $ARGUMENTS
