The peer-reviewed, revised plan is ready for your decision before any code is written.

**Feature:** {{ goal }}

The full plan is in the previous step's output (and at `/tmp/plan.md` in the sandbox), including the "Changes from review" summary and the native-client follow-up section.

**Approve this plan and proceed to implementation?**

- **Yes** → the implementer builds the in-scope work (CI-verifiable Rust + web), then it goes through adversarial code review and the CI gate before a PR is opened.
- **No** → the run stops here with the plan captured, so you can adjust the goal or steer and re-run.

(Tip: launch with `--auto-approve` to skip this gate for a fully autonomous run.)
