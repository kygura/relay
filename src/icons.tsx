import type { ReactNode, SVGProps } from 'react';

type IconProps = SVGProps<SVGSVGElement> & { size?: number };

function Base({ size = 16, children, ...rest }: IconProps & { children: ReactNode }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.8}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
      {...rest}
    >
      {children}
    </svg>
  );
}

export const IconMic = (p: IconProps) => (
  <Base {...p}>
    <rect x="9" y="3" width="6" height="11" rx="3" />
    <path d="M5 11a7 7 0 0 0 14 0" />
    <path d="M12 18v3" />
  </Base>
);

export const IconSpeaker = (p: IconProps) => (
  <Base {...p}>
    <path d="M4 9h4l5-4v14l-5-4H4z" />
    <path d="M16.5 9a4 4 0 0 1 0 6" />
    <path d="M19 6.5a8 8 0 0 1 0 11" />
  </Base>
);

export const IconRecord = (p: IconProps) => (
  <Base {...p}>
    <circle cx="12" cy="12" r="6" fill="currentColor" />
  </Base>
);

export const IconStop = (p: IconProps) => (
  <Base {...p}>
    <rect x="6" y="6" width="12" height="12" rx="2" fill="currentColor" />
  </Base>
);

export const IconOpen = (p: IconProps) => (
  <Base {...p}>
    <path d="M14 4h6v6" />
    <path d="M20 4l-9 9" />
    <path d="M18 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1h5" />
  </Base>
);

export const IconHide = (p: IconProps) => (
  <Base {...p}>
    <path d="M5 12h14" />
  </Base>
);

export const IconClose = (p: IconProps) => (
  <Base {...p}>
    <path d="M6 6l12 12" />
    <path d="M18 6L6 18" />
  </Base>
);

export const IconCheck = (p: IconProps) => (
  <Base {...p}>
    <path d="M5 12.5l4.5 4.5L19 7.5" />
  </Base>
);

export const IconAlert = (p: IconProps) => (
  <Base {...p}>
    <path d="M12 4l9 16H3z" />
    <path d="M12 10v4" />
    <path d="M12 17.5v.01" />
  </Base>
);

export const IconShield = (p: IconProps) => (
  <Base {...p}>
    <path d="M12 3l7 3v5c0 4.5-3 8-7 10-4-2-7-5.5-7-10V6z" />
  </Base>
);

export const IconCopy = (p: IconProps) => (
  <Base {...p}>
    <rect x="9" y="9" width="11" height="11" rx="2" />
    <path d="M5 15V6a2 2 0 0 1 2-2h9" />
  </Base>
);

export const IconInsert = (p: IconProps) => (
  <Base {...p}>
    <path d="M4 5h16" />
    <path d="M4 10h10" />
    <path d="M12 17h8" />
    <path d="M16 13l4 4-4 4" />
  </Base>
);

export const IconRefresh = (p: IconProps) => (
  <Base {...p}>
    <path d="M20 11a8 8 0 0 0-14-4L4 9" />
    <path d="M4 4v5h5" />
    <path d="M4 13a8 8 0 0 0 14 4l2-2" />
    <path d="M20 20v-5h-5" />
  </Base>
);

export const IconSend = (p: IconProps) => (
  <Base {...p}>
    <path d="M4 12l16-8-6 16-2-7z" />
  </Base>
);

export const IconDownload = (p: IconProps) => (
  <Base {...p}>
    <path d="M12 4v11" />
    <path d="M7 11l5 5 5-5" />
    <path d="M5 20h14" />
  </Base>
);

export const IconTrash = (p: IconProps) => (
  <Base {...p}>
    <path d="M5 7h14" />
    <path d="M10 7V4h4v3" />
    <path d="M7 7l1 13h8l1-13" />
  </Base>
);

export const IconLive = (p: IconProps) => (
  <Base {...p}>
    <path d="M4 12h3l2-6 4 12 2-6h5" />
  </Base>
);

export const IconNotes = (p: IconProps) => (
  <Base {...p}>
    <rect x="5" y="3" width="14" height="18" rx="2" />
    <path d="M9 8h6" />
    <path d="M9 12h6" />
    <path d="M9 16h3" />
  </Base>
);

export const IconAgent = (p: IconProps) => (
  <Base {...p}>
    <rect x="5" y="8" width="14" height="11" rx="3" />
    <path d="M12 4v4" />
    <path d="M9 13.5v.01" />
    <path d="M15 13.5v.01" />
  </Base>
);

export const IconSettings = (p: IconProps) => (
  <Base {...p}>
    <path d="M4 7h10" />
    <path d="M18 7h2" />
    <circle cx="16" cy="7" r="2" />
    <path d="M4 17h2" />
    <path d="M10 17h10" />
    <circle cx="8" cy="17" r="2" />
  </Base>
);

export const IconHistory = (p: IconProps) => (
  <Base {...p}>
    <path d="M3 12a9 9 0 1 0 3-6.7" />
    <path d="M3 4v4h4" />
    <path d="M12 7v5l3 2" />
  </Base>
);

export const IconEdit = (p: IconProps) => (
  <Base {...p}>
    <path d="M4 20h4L19 9a2.1 2.1 0 0 0-3-3L5 17z" />
    <path d="M14 7l3 3" />
  </Base>
);

export const IconPlay = (p: IconProps) => (
  <Base {...p}>
    <path d="M7 5v14l11-7z" fill="currentColor" />
  </Base>
);

export const IconPause = (p: IconProps) => (
  <Base {...p}>
    <rect x="6.5" y="5" width="3.5" height="14" rx="1" fill="currentColor" stroke="none" />
    <rect x="14" y="5" width="3.5" height="14" rx="1" fill="currentColor" stroke="none" />
  </Base>
);

export const IconSearch = (p: IconProps) => (
  <Base {...p}>
    <circle cx="11" cy="11" r="6.5" />
    <path d="M20 20l-4.2-4.2" />
  </Base>
);

export const IconArrowDown = (p: IconProps) => (
  <Base {...p}>
    <path d="M12 5v14" />
    <path d="M6 13l6 6 6-6" />
  </Base>
);
