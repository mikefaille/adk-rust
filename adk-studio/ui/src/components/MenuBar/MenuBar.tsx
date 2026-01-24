import { useState, useRef, useEffect } from 'react';
import { useStore } from '../../store';
import { TEMPLATES, Template } from './templates';
import { useTheme } from '../../hooks/useTheme';

interface MenuBarProps {
  onExportCode: () => void;
  onNewProject: () => void;
  onTemplateApplied?: () => void;
}

export function MenuBar({ onExportCode, onNewProject, onTemplateApplied }: MenuBarProps) {
  const [openMenu, setOpenMenu] = useState<string | null>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const { currentProject, addAgent, removeAgent, addEdge, removeEdge } = useStore();
  const { mode } = useTheme();
  const isLight = mode === 'light';

  useEffect(() => {
    const handleClickOutside = (e: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        setOpenMenu(null);
      }
    };
    document.addEventListener('mousedown', handleClickOutside);
    return () => document.removeEventListener('mousedown', handleClickOutside);
  }, []);

  const applyTemplate = (template: Template) => {
    if (!currentProject) return;

    // Clear existing edges
    (currentProject.workflow?.edges || []).forEach(e => removeEdge(e.from, e.to));

    // Clear existing agents
    Object.keys(currentProject.agents).forEach(id => removeAgent(id));

    // Add all agents from template
    Object.entries(template.agents).forEach(([id, agent]) => {
      addAgent(id, agent);
    });

    // Add edges from template
    template.edges.forEach(e => addEdge(e.from, e.to));

    if (onTemplateApplied) {
      onTemplateApplied();
    }

    setOpenMenu(null);
  };

  const menuButtonClass = isLight
    ? 'hover:bg-gray-200'
    : 'hover:bg-gray-700';
  
  const menuActiveClass = isLight
    ? 'bg-gray-200'
    : 'bg-gray-700';

  const Menu = ({ name, children }: { name: string; children: React.ReactNode }) => (
    <div className="relative">
      <button
        className={`px-3 py-1 text-sm rounded ${menuButtonClass} ${openMenu === name ? menuActiveClass : ''}`}
        style={{ color: 'var(--text-primary)' }}
        onClick={() => setOpenMenu(openMenu === name ? null : name)}
      >
        {name}
      </button>
      {openMenu === name && (
        <div 
          className="absolute top-full left-0 mt-1 rounded shadow-lg min-w-[200px] z-50"
          style={{ backgroundColor: 'var(--surface-panel)', border: '1px solid var(--border-default)' }}
        >
          {children}
        </div>
      )}
    </div>
  );

  const MenuItem = ({ onClick, children, disabled }: { onClick: () => void; children: React.ReactNode; disabled?: boolean }) => (
    <button
      className={`w-full text-left px-3 py-2 text-sm ${menuButtonClass} ${disabled ? 'opacity-50 cursor-not-allowed' : ''}`}
      style={{ color: 'var(--text-primary)' }}
      onClick={() => { if (!disabled) { onClick(); setOpenMenu(null); } }}
      disabled={disabled}
    >
      {children}
    </button>
  );

  const Divider = () => <div className="my-1" style={{ borderTop: '1px solid var(--border-default)' }} />;

  return (
    <div 
      ref={menuRef} 
      className="flex items-center gap-1 px-2 py-1"
      style={{ backgroundColor: 'var(--surface-panel)', borderBottom: '1px solid var(--border-default)' }}
    >
      <span className="text-sm font-semibold mr-4" style={{ color: 'var(--accent-primary)' }}>🔧 ADK Studio</span>

      <Menu name="File">
        <MenuItem onClick={onNewProject}>📄 New Project</MenuItem>
        <Divider />
        <MenuItem onClick={onExportCode} disabled={!currentProject}>📦 Export Code</MenuItem>
      </Menu>

      <Menu name="Templates">
        <div className="px-3 py-1 text-xs" style={{ color: 'var(--text-muted)', borderBottom: '1px solid var(--border-default)' }}>Add to current project</div>
        {TEMPLATES.map(t => (
          <MenuItem key={t.id} onClick={() => applyTemplate(t)} disabled={!currentProject}>
            {t.icon} {t.name}
          </MenuItem>
        ))}
      </Menu>

      <Menu name="Help">
        <MenuItem onClick={() => window.open('https://github.com/zavora-ai/adk-rust', '_blank')}>📚 Documentation</MenuItem>
        <Divider />
        <div className="px-3 py-2 text-xs" style={{ color: 'var(--text-secondary)' }}>
          <div className="font-semibold mb-1">Keyboard Shortcuts</div>
          <div>Drag agents from left panel</div>
          <div>Click agent to edit properties</div>
          <div>Drag tools onto agents</div>
        </div>
        <Divider />
        <div className="px-3 py-2 text-xs" style={{ color: 'var(--text-muted)' }}>ADK Studio v0.1.0</div>
      </Menu>

      <div className="flex-1" />

      {currentProject && (
        <span className="text-sm" style={{ color: 'var(--text-secondary)' }}>
          Project: <span style={{ color: 'var(--text-primary)' }}>{currentProject.name}</span>
        </span>
      )}
    </div>
  );
}
