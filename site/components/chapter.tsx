import { Status, type Tone } from "./status";
import { Reveal } from "./reveal";

type Props = {
  tone: Tone;
  status: string;
  title: string;
  children: React.ReactNode;
  as?: "article" | "div";
};

/** One step of the run: its state sticks in the left rail while you read it. */
export function Chapter({ tone, status, title, children, as: Tag = "article" }: Props) {
  return (
    <Tag className="chapter wrap">
      <div className="rail">
        <div className="state">
          <span className="node" />
          <Status tone={tone}>{status}</Status>
        </div>
      </div>
      <div className="body">
        <Reveal variant="heading"><h3>{title}</h3></Reveal>
        {children}
      </div>
    </Tag>
  );
}
