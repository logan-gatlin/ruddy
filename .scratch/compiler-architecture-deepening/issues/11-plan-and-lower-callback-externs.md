# 11: Plan and lower callback externs

**What to build:** Extend `ExternPlan` across callback capabilities, evidence conversion, and recursive adapter cases, then remove duplicated target-neutral interpretation from LIR.

**Blocked by:** 10/Plan and lower ordinary externs

**Status:** ready-for-agent

- [ ] Callback host-to-Ruddy and Ruddy-to-host conversion is represented in `ExternPlan`.
- [ ] Callback capability facts used for inference diagnostics produce the corresponding lowering plan without a second semantic walk.
- [ ] Recursive adapters and adapter identity preserve current behavior.
- [ ] Extern planning is infallible for accepted target-neutral facts.
- [ ] `ExternPlan` is absorbed into LIR and is not serialized in artifacts.
- [ ] Duplicated target-neutral extern analysis is removed from LIR.
- [ ] The complete `just test` suite passes.

