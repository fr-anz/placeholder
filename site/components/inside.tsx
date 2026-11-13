import type { CSSProperties } from "react";
import { SPEC } from "@/lib/site";
import { Reveal } from "./reveal";

export function Inside() {
  return (
    <section className="inside wrap" id="inside">
      <Reveal className="sec-head" variant="heading">
        <h2>The tools you need, built in.</h2>
      </Reveal>
      <Reveal as="dl" className="spec" stagger>
        {SPEC.map(({ term, detail }, index) => (
          <div key={term} style={{ "--reveal-order": index } as CSSProperties}>
            <dt>{term}</dt>
            <dd>{detail}</dd>
          </div>
        ))}
      </Reveal>
    </section>
  );
}
