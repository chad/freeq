I can't write this accurately.

No code was written or modified in this conversation. The session so far has been entirely the pasted spec/CLAUDE.md document followed by this request. There is no `git diff`, no test file, and no changes for me to base a PR description on.

If I wrote a PR description anyway, I would be fabricating specifics — file names, test names, behaviors covered, clippy pass status — none of which I can verify. The Notes section of your own template says "do not claim anything you didn't verify," and that's the right bar.

**To get an accurate PR description, you can:**

1. Run `git diff --stat HEAD~1` and `git diff HEAD~1` and paste the output here — I'll write the description strictly from that.
2. Or describe what tests were added (which file, which cases) and I'll structure it correctly from that input.

I won't invent content to fill the template.