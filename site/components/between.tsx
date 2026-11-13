import automations from "@/assets/shots/automations-m.webp";
import minion from "@/assets/shots/minion.webp";
import visuals from "@/assets/shots/visuals-m.webp";
import { Shot } from "./shot";
import { Reveal } from "./reveal";

/** What the app is for when no run is going. */
export function Between() {
  return (
    <section className="between wrap" id="between">
      <Reveal className="sec-head" variant="heading">
        <h2>Beyond the run.</h2>
      </Reveal>

      <article className="feature">
        <Reveal className="feature-text" stagger>
          <h3>Ask a question. Get a chart.</h3>
          <p>
            Explore charts and diagrams right inside the conversation.
          </p>
        </Reveal>
        <Shot
          src={visuals}
          alt="An answer about deploys that contains a bar chart of deploys per week, two trend statistics and a gauge reading 82 percent runner capacity."
        />
      </article>

      <article className="feature flip">
        <Reveal className="feature-text" stagger>
          <h3>A Minion is an agent that stays.</h3>
          <p>
            An always-on agent with memory, project context, and scheduled check-ins.
          </p>
        </Reveal>
        <Shot
          src={minion}
          alt="The settings of a Minion named Franz, shown as a small pixel-art character, with General, Soul and Memory pages and its model, reasoning and permissions."
        />
      </article>

      <article className="feature">
        <Reveal className="feature-text" stagger>
          <h3>Put the boring work on a schedule.</h3>
          <p>
            Schedule reviews, security scans, and the work you repeat.
          </p>
        </Reveal>
        <Shot
          src={automations}
          alt="The New automation screen with example cards: Find critical bugs, Scan codebase for vulnerabilities, Generate docs and Add test coverage, each with a schedule."
        />
      </article>
    </section>
  );
}
