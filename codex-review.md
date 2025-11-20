# Generational Arena Hybrid Review

## Quick Take
- The arena-plus-refcount direction targets the clone-heavy bottleneck called out in the current architecture notes and is a prerequisite for CPython-compatible identity semantics.
- Several critical details are still unspecified though, and without them the plan risks swapping the current inefficiencies for memory unsafety, runaway heap growth, or duplicated work when closures/scopes arrive later.

## Heap & Refcount Safety
1. **Reference counts lack a safe owner** – `Object` still derives `Clone`/`PartialEq` (generational-arena-hybrid.md:92), while evaluation relies heavily on implicit cloning through `Cow<Object>` (how-it-works.md:1002). With the proposed shape each `Object::Ref` clone just copies an `ObjectId`, so `inc_ref`/`dec_ref` never run and aliasing either leaks or double-frees. Remove the auto-derives, add a handle type that knows the heap (`struct HeapRef { id, heap }`) or funnel every clone/drop through helper APIs so refcounts stay correct.
2. **Heap copies break identity guarantees** – The executor clones the heap at the start of each run (generational-arena-hybrid.md:318) and every `RunFrame` owns a `Heap` (generational-arena-hybrid.md:356), so function calls would duplicate every live object, undermining identity and O(1) assignment. Keep a single heap per execution (shared mutably or behind `Rc<RefCell<_>>`) and make frames borrow it instead of owning clones; otherwise the arena design never actually shares objects.

## Memory Management Risks
3. **Arena memory never shrinks in practice** – Storing entries inside `Vec<Option<HeapObject>>` (generational-arena-hybrid.md:111) means freed slots still occupy a full `HeapObject`, not “1 byte” as the trade-off section claims (generational-arena-hybrid.md:79). Without a real compaction/clear story (generational-arena-hybrid.md:221) long-running shells will retain every peak allocation. Add `Heap::clear` hooks between executions and schedule the compacting GC from Phase 1, not as a distant enhancement.
4. **Cycle-heavy Python patterns still leak indefinitely** – The plan explicitly punts on circular reference collection (generational-arena-hybrid.md:747) even though Python code constantly builds cycles via closures, class graphs, and even default arguments (how-it-works.md:1294). Refcounts alone will never drop to zero here, so the interpreter will leak per run until process exit. Treat mark-sweep or trial deletion as in-scope for this milestone so we can run real programs without exhausting memory.

## Integration with Broader Semantics
5. **Hybrid heap doesn’t address LEGB scopes or owned ASTs** – Monty still has flat namespaces and borrowed AST lifetimes (how-it-works.md:1262, how-it-works.md:1290), and the hybrid plan doesn’t show how heap IDs integrate with those upcoming changes (generational-arena-hybrid.md:747). Pair this work with a concrete scope-chain design (enclosing/global/builtin stacks) so we don’t need to rethread every evaluation path first for the heap and then again for closures, and plan for owned AST storage so `eval/exec` stop blocking on `'c` lifetimes.
