/** The app draws its charts with an ordered dither, so the page changes tone
    the same way: four strips at 12, 25, 50 and 75 percent ink. */
export function Dither({ flip }: { flip?: boolean }) {
  return (
    <div className={flip ? "dither flip" : "dither"} aria-hidden="true">
      <i />
      <i />
      <i />
      <i />
    </div>
  );
}
