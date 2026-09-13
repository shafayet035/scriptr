import type { Plan, Script } from "./types";

/**
 * Kahn's algorithm into waves over the given scripts. Dependencies on scripts outside the
 * set are ignored for ordering. Mirrors src-tauri/src/graph.rs so the UI can preview a
 * plan instantly (e.g. while dragging an edge) without a round trip.
 */
export function computePlan(groupId: string, scripts: Script[]): Plan {
  const ids = new Set(scripts.map((s) => s.id));
  const indegree = new Map<string, number>();
  const dependents = new Map<string, string[]>();
  for (const s of scripts) {
    indegree.set(s.id, 0);
    dependents.set(s.id, []);
  }
  for (const s of scripts) {
    for (const dep of s.after) {
      if (!ids.has(dep) || dep === s.id) continue;
      indegree.set(s.id, (indegree.get(s.id) ?? 0) + 1);
      dependents.get(dep)!.push(s.id);
    }
  }

  const order = new Map(scripts.map((s, i) => [s.id, i]));
  const byOrder = (a: string, b: string) => (order.get(a) ?? 0) - (order.get(b) ?? 0);

  const waves: string[][] = [];
  let frontier = scripts.filter((s) => indegree.get(s.id) === 0).map((s) => s.id);
  let placed = 0;
  while (frontier.length > 0) {
    frontier.sort(byOrder);
    waves.push(frontier);
    placed += frontier.length;
    const next: string[] = [];
    for (const id of frontier) {
      for (const d of dependents.get(id)!) {
        const n = indegree.get(d)! - 1;
        indegree.set(d, n);
        if (n === 0) next.push(d);
      }
    }
    frontier = next;
  }

  if (placed === scripts.length) return { groupId, waves, cycle: null };
  return { groupId, waves, cycle: findCycleEdge(scripts, ids) };
}

/** Returns one edge [dependency, dependent] that closes a cycle. */
function findCycleEdge(scripts: Script[], ids: Set<string>): [string, string] | null {
  const byId = new Map(scripts.map((s) => [s.id, s]));
  const color = new Map<string, 0 | 1 | 2>();
  let found: [string, string] | null = null;

  const visit = (id: string) => {
    color.set(id, 1);
    for (const dep of byId.get(id)!.after) {
      if (found || !ids.has(dep)) continue;
      const c = color.get(dep) ?? 0;
      if (c === 1) {
        found = [dep, id];
        return;
      }
      if (c === 0) visit(dep);
    }
    color.set(id, 2);
  };
  for (const s of scripts) {
    if (!found && (color.get(s.id) ?? 0) === 0) visit(s.id);
  }
  return found;
}

/** Would making `script` wait for `dep` create a cycle? */
export function wouldCycle(scripts: Script[], scriptId: string, dep: string): boolean {
  const next = scripts.map((s) => (s.id === scriptId ? { ...s, after: [...new Set([...s.after, dep])] } : s));
  return computePlan("", next).cycle !== null;
}
