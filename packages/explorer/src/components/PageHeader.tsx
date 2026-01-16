import { Link } from 'react-router-dom';
import { useTheme } from '../hooks/useTheme';

interface PageHeaderProps {
  showThemeToggle?: boolean;
}

export function PageHeader({ showThemeToggle = true }: PageHeaderProps) {
  const { darkMode, toggleTheme } = useTheme();

  return (
    <header style={{ position: 'relative' }}>
      <Link to="/" style={{ textDecoration: 'none', color: 'inherit' }}>
        <h1>rinku explorer</h1>
      </Link>
      <p>url-native distributed ledger</p>
      {showThemeToggle && (
        <div className="header-actions">
          <button className="theme-toggle" onClick={toggleTheme}>
            {darkMode ? '☀' : '☽'}
          </button>
        </div>
      )}
    </header>
  );
}
