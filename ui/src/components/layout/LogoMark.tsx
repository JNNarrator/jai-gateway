/**
 * JAI 品牌标识：蓝紫渐变圆角方块 + 白色 J + AI 星火。
 * 与 src-tauri/icons 同源（主稿 ui/public/jai-logo.svg），改一处需同步重生成图标。
 */
export function LogoMark({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 1024 1024" className={className} aria-hidden="true">
      <defs>
        <linearGradient id="jai-logo-bg" x1="0" y1="0" x2="1" y2="1">
          <stop offset="0" stopColor="#6D6AF0" />
          <stop offset="0.5" stopColor="#4F46E5" />
          <stop offset="1" stopColor="#7C3AED" />
        </linearGradient>
        <radialGradient id="jai-logo-glow" cx="0.26" cy="0.18" r="0.95">
          <stop offset="0" stopColor="#C7D2FE" stopOpacity="0.5" />
          <stop offset="0.55" stopColor="#C7D2FE" stopOpacity="0" />
        </radialGradient>
      </defs>
      <rect width="1024" height="1024" rx="232" fill="url(#jai-logo-bg)" />
      <rect width="1024" height="1024" rx="232" fill="url(#jai-logo-glow)" />
      <path
        d="M 784 116 C 802 240 830 268 954 286 C 830 304 802 332 784 456 C 766 332 738 304 614 286 C 738 268 766 240 784 116 Z"
        fill="#C7D2FE"
      />
      <path
        d="M 300 220 C 310 274 324 288 378 298 C 324 308 310 322 300 376 C 290 322 276 308 222 298 C 276 288 290 274 300 220 Z"
        fill="#C7D2FE"
        opacity="0.9"
      />
      <path
        d="M 818 682 C 825 722 835 732 875 739 C 835 746 825 756 818 796 C 811 756 801 746 761 739 C 801 732 811 722 818 682 Z"
        fill="#C7D2FE"
        opacity="0.75"
      />
      <path
        d="M 656 318 L 656 614 C 656 742 566 802 462 802 C 380 802 322 762 296 698"
        fill="none"
        stroke="#FFFFFF"
        strokeWidth="116"
        strokeLinecap="round"
      />
    </svg>
  );
}
