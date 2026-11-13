type Props = { os: "Linux" | "Windows" | "macOS" };

/** Small, local SVG platform marks; no icon font or remote assets. */
export function PlatformIcon({ os }: Props) {
  return (
    <svg className="platform-icon" viewBox="0 0 48 48" fill="currentColor" aria-hidden="true" focusable="false">
      {os === "Windows" ? (
        <path d="M5 9.5 22 7v16H5V9.5Zm20-3L43 4v19H25V6.5ZM5 26h17v15L5 38.5V26Zm20 0h18v18l-18-2.5V26Z" />
      ) : os === "macOS" ? (
        <path d="M32.1 3c.3 3-1 5.9-2.8 7.9-1.9 2-4.7 3.5-7.4 3.3-.4-2.9 1-5.8 2.7-7.7C26.5 4.5 29.5 3.1 32.1 3ZM38.2 34c-1.1 2.5-1.7 3.7-3.1 5.8-2 2.9-4.8 6.4-8.2 6.4-3 0-3.8-2-7.9-2-4 0-4.9 2-7.9 2-3.3 0-5.9-3.2-7.9-6.1C-2.3 32.1-2.8 21.4.8 16c2.5-3.8 6.5-6 10.2-6 3.8 0 6.1 2 9.2 2 3 0 4.8-2 9.2-2 3.3 0 6.9 1.8 9.4 4.9-8.2 4.5-6.9 16.2-.6 19.1Z" transform="translate(6 2) scale(.85)" />
      ) : (
        <>
          <path d="M24 4c-6.5 0-9 5.5-8.5 11.5.4 4.6-2.6 6.7-5.1 11.1-2.8 4.9-3.8 11.4 1.1 14.3 3.4 2 7.7 1.3 12.5 1.3s9.1.7 12.5-1.3c4.9-2.9 3.9-9.4 1.1-14.3-2.5-4.4-5.5-6.5-5.1-11.1C33 9.5 30.5 4 24 4Z" />
          <path d="M24 20c-5.2 0-9.1 7-9.1 13.1 0 4.8 3.8 7.1 9.1 7.1s9.1-2.3 9.1-7.1C33.1 27 29.2 20 24 20Z" fill="#f4f3ef" />
          <ellipse cx="20" cy="15" rx="2.7" ry="3.6" fill="#f4f3ef" />
          <ellipse cx="28" cy="15" rx="2.7" ry="3.6" fill="#f4f3ef" />
          <ellipse cx="20.6" cy="15.8" rx="1" ry="1.6" />
          <ellipse cx="27.4" cy="15.8" rx="1" ry="1.6" />
          <path d="m18.5 20 5.5-3 5.5 3-5.5 4-5.5-4ZM7 38l8-3 5 7-12 3-3-3 2-4Zm34 0-8-3-5 7 12 3 3-3-2-4Z" fill="#f6a823" />
        </>
      )}
    </svg>
  );
}
