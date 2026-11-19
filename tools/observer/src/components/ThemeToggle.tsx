import { Button } from 'antd';
import { Moon, Sun } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { useCraftTheme } from './CraftThemeProvider';

export function ThemeToggle() {
  const { t } = useTranslation();
  const { mode, toggle } = useCraftTheme();
  const dark = mode === 'dark';
  const Icon = dark ? Sun : Moon;
  const label = t(dark ? 'shell.switchToLight' : 'shell.switchToDark');
  return (
    <div className="shell-theme">
      <Button
        block
        type="text"
        className="shell-theme-toggle"
        data-testid="theme-toggle"
        aria-label={label}
        icon={<Icon size={16} strokeWidth={1.5} aria-hidden />}
        onClick={toggle}
      >
        <span className="shell-theme-label">{label}</span>
      </Button>
    </div>
  );
}
