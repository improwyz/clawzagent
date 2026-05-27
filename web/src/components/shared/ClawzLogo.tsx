import { clsx } from 'clsx';

const LOGO_SRC = {
  full: {
    dark: '/branding/clawz-logo-dark.png',
    light: '/branding/clawz-logo-light.png',
  },
  mark: {
    dark: '/branding/clawz-mark-dark.png',
    light: '/branding/clawz-mark-light.png',
  },
} as const;

export interface ClawzLogoProps {
  variant?: 'full' | 'mark';
  theme?: 'dark' | 'light';
  className?: string;
}

export function ClawzLogo({
  variant = 'full',
  theme = 'dark',
  className,
}: ClawzLogoProps) {
  return (
    <img
      src={LOGO_SRC[variant][theme]}
      alt="ClawZ"
      className={clsx('object-contain', className)}
    />
  );
}
