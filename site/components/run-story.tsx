import board from "@/assets/shots/board.webp";
import boardNarrow from "@/assets/shots/board-m.webp";
import diff from "@/assets/shots/diff.webp";
import diffNarrow from "@/assets/shots/diff-m.webp";
import orchestrate from "@/assets/shots/orchestrate.webp";
import orchestrateNarrow from "@/assets/shots/orchestrate-m.webp";
import { Chapter } from "./chapter";
import { Dither } from "./dither";
import { RunGraph } from "./run-graph";
import { Shot } from "./shot";
import { Status } from "./status";
import { Reveal } from "./reveal";

/** One card, followed from the backlog to the branch you review. */
export function RunStory() {
  return (
    <>
      <section className="story" id="run">
        <Reveal className="wrap story-head" variant="heading">
          <h2>Follow one card.</h2>
        </Reveal>

        <Chapter tone="idle" status="Not started" title="Start with a task.">
          <p className="chapter-copy">
            Describe the work. Placeholder turns it into a card and tracks it through review.
          </p>
          <Shot
            src={board}
            narrow={boardNarrow}
            alt="The project board with Backlog, In progress and Needs review lanes. Card HAR-009, Signed webhooks with replay protection, is in progress with four agents."
          />
        </Chapter>

        <Chapter tone="plan" status="Plan ready" title="Approve the plan.">
          <p className="chapter-copy">
            A coordinator splits the task into assignments. You approve the plan before agents start.
          </p>
          <Shot
            src={orchestrate}
            narrow={orchestrateNarrow}
            alt="A coordinator session listing four numbered assignments. In the sidebar, four worker agents are nested under the session, one marked Done and one Queued."
          />
        </Chapter>
      </section>

      <Dither />

      <section className="pipe" id="pipeline">
        <Chapter
          as="div"
          tone="run"
          status="Workers running"
          title="Work in parallel."
        >
          <p className="chapter-copy">
            Each agent gets its own worktree. Up to four run at once, without touching your branch.
          </p>

          <RunGraph />

          <div className="trio">
            <div>
              <p>
                <Status tone="rev">In review</Status>
              </p>
              <p>
                An independent agent checks each assignment.
              </p>
            </div>
            <div>
              <p>
                <Status tone="ok">Merging</Status>
              </p>
              <p>
                Changes merge in order, with checks after each.
              </p>
            </div>
            <div>
              <p>
                <Status tone="run">Asking orchestrator</Status>
              </p>
              <p>
                A decider handles retries and blockers.
              </p>
            </div>
          </div>
        </Chapter>
      </section>

      <Dither flip />

      <section className="story story-end">
        <Chapter tone="ok" status="Finished" title="Review one branch.">
          <p className="chapter-copy">
            Read the changes beside the conversation that produced them. Commit when you’re ready.
          </p>
          <Shot
            src={diff}
            narrow={diffNarrow}
            alt="A split view: the conversation on the left, a red and green diff of rate-limit.ts in the middle, and the list of changed files with a Commit button on the right."
          />
        </Chapter>
      </section>
    </>
  );
}
