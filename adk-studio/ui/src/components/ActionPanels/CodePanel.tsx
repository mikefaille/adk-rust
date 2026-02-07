/**
 * CodePanel Component for ADK Studio
 * 
 * Properties panel for configuring Code action nodes.
 * Provides UI for language selection, code editing with Monaco editor,
 * and sandbox security configuration.
 * 
 * Requirements: 10.1, 10.2, 10.3, 12.2
 */

import React, { useCallback, useState, useRef, useEffect } from 'react';
import { StandardPropertiesPanel } from './StandardPropertiesPanel';
import { CollapsibleSection } from '../shared/CollapsibleSection';
import { Field } from '../shared/Field';
import type { 
  CodeNodeConfig, 
  CodeLanguage,
  SandboxConfig,
} from '../../types/actionNodes';
import type { StandardProperties } from '../../types/standardProperties';
import '../../styles/codePanel.css';

// ============================================
// Constants
// ============================================

const CODE_LANGUAGES: readonly CodeLanguage[] = ['javascript', 'typescript'];

const LANGUAGE_CONFIG: Record<CodeLanguage, {
  label: string;
  description: string;
  icon: string;
  defaultCode: string;
}> = {
  javascript: {
    label: 'JavaScript',
    description: 'Standard JavaScript (ES2020+)',
    icon: '📜',
    defaultCode: `// Access input data via 'input' variable
// Return the transformed result

const result = input.data.map(item => ({
  ...item,
  processed: true
}));

return result;`,
  },
  typescript: {
    label: 'TypeScript',
    description: 'TypeScript (transpiled to JS)',
    icon: '📘',
    defaultCode: `// Access input data via 'input' variable
// Return the transformed result

interface Item {
  id: string;
  name: string;
}

const result: Item[] = input.data.map((item: Item) => ({
  ...item,
  processed: true
}));

return result;`,
  },
};

// Default sandbox configuration
const DEFAULT_SANDBOX: SandboxConfig = {
  networkAccess: false,
  fileSystemAccess: false,
  memoryLimit: 128,
  timeLimit: 5000,
};

// ============================================
// Main Component
// ============================================

export interface CodePanelProps {
  /** Current Code node configuration */
  node: CodeNodeConfig;
  /** Callback when configuration changes */
  onChange: (node: CodeNodeConfig) => void;
}

/**
 * CodePanel provides configuration UI for Code action nodes.
 * 
 * Features:
 * - Language selector (JavaScript/TypeScript) (Requirement 10.1)
 * - Code editor with syntax highlighting (Requirement 10.3)
 * - Sandbox security configuration (Requirement 10.2)
 * - Input/output type hints
 * - Standard properties panel integration
 * 
 * @see Requirements 10.1, 10.2, 10.3, 12.2
 */
export function CodePanel({ node, onChange }: CodePanelProps) {
  
  // ============================================
  // Update Handlers
  // ============================================
  
  const updateLanguage = useCallback((language: CodeLanguage) => {
    onChange({ ...node, language });
  }, [node, onChange]);
  
  const updateCode = useCallback((code: string) => {
    onChange({ ...node, code });
  }, [node, onChange]);
  
  const updateSandbox = useCallback((updates: Partial<SandboxConfig>) => {
    onChange({
      ...node,
      sandbox: { ...node.sandbox, ...updates },
    });
  }, [node, onChange]);
  
  const updateInputType = useCallback((inputType: string | undefined) => {
    if (inputType === undefined || inputType === '') {
      const { inputType: _, ...rest } = node;
      onChange(rest as CodeNodeConfig);
    } else {
      onChange({ ...node, inputType });
    }
  }, [node, onChange]);
  
  const updateOutputType = useCallback((outputType: string | undefined) => {
    if (outputType === undefined || outputType === '') {
      const { outputType: _, ...rest } = node;
      onChange(rest as CodeNodeConfig);
    } else {
      onChange({ ...node, outputType });
    }
  }, [node, onChange]);
  
  const updateStandardProperties = useCallback((props: StandardProperties) => {
    onChange({ ...node, ...props });
  }, [node, onChange]);
  
  const handleInsertTemplate = useCallback(() => {
    const config = LANGUAGE_CONFIG[node.language];
    updateCode(config.defaultCode);
  }, [node.language, updateCode]);
  
  // ============================================
  // Render
  // ============================================
  
  return (
    <div className="code-panel">
      {/* Language Selection (Requirement 10.1) */}
      <CollapsibleSection title="Language" defaultOpen>
        <div className="code-language-selector">
          {CODE_LANGUAGES.map((lang) => {
            const config = LANGUAGE_CONFIG[lang];
            return (
              <button
                key={lang}
                type="button"
                className={`code-language-option ${node.language === lang ? 'selected' : ''}`}
                onClick={() => updateLanguage(lang)}
              >
                <span className="code-language-icon">{config.icon}</span>
                <span className="code-language-label">{config.label}</span>
                <span className="code-language-description">{config.description}</span>
              </button>
            );
          })}
        </div>
      </CollapsibleSection>
      
      {/* Code Editor (Requirement 10.3) */}
      <CodeEditorSection
        language={node.language}
        code={node.code}
        onChange={updateCode}
        onInsertTemplate={handleInsertTemplate}
      />
      
      {/* Type Hints */}
      <TypeHintsSection
        inputType={node.inputType}
        outputType={node.outputType}
        onInputTypeChange={updateInputType}
        onOutputTypeChange={updateOutputType}
      />
      
      {/* Sandbox Configuration (Requirement 10.2) */}
      <SandboxSection
        sandbox={node.sandbox}
        onChange={updateSandbox}
      />
      
      {/* Standard Properties */}
      <StandardPropertiesPanel
        properties={node}
        onChange={updateStandardProperties}
        showIdentity
      />
    </div>
  );
}

// ============================================
// Code Editor Section Component
// ============================================

interface CodeEditorSectionProps {
  language: CodeLanguage;
  code: string;
  onChange: (code: string) => void;
  onInsertTemplate: () => void;
}

/**
 * Code editor section with syntax highlighting hints.
 * @see Requirement 10.3
 */
function CodeEditorSection({ language, code, onChange, onInsertTemplate }: CodeEditorSectionProps) {
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const [lineCount, setLineCount] = useState(1);
  
  // Update line count when code changes
  useEffect(() => {
    const lines = code.split('\n').length;
    setLineCount(Math.max(lines, 1));
  }, [code]);
  
  // Handle tab key for indentation
  const handleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === 'Tab') {
      e.preventDefault();
      const textarea = textareaRef.current;
      if (!textarea) return;
      
      const start = textarea.selectionStart;
      const end = textarea.selectionEnd;
      const newCode = code.substring(0, start) + '  ' + code.substring(end);
      onChange(newCode);
      
      // Set cursor position after the inserted spaces
      setTimeout(() => {
        textarea.selectionStart = textarea.selectionEnd = start + 2;
      }, 0);
    }
  };
  
  return (
    <CollapsibleSection title="Code" defaultOpen>
      <div className="code-editor-container">
        {/* Toolbar */}
        <div className="code-editor-toolbar">
          <span className="code-editor-language-badge">
            {LANGUAGE_CONFIG[language].icon} {LANGUAGE_CONFIG[language].label}
          </span>
          <div className="code-editor-actions">
            <button
              type="button"
              className="code-editor-action"
              onClick={onInsertTemplate}
              title="Insert template code"
            >
              📋 Template
            </button>
            <button
              type="button"
              className="code-editor-action"
              onClick={() => onChange('')}
              title="Clear code"
            >
              🗑️ Clear
            </button>
          </div>
        </div>
        
        {/* Editor with line numbers */}
        <div className="code-editor-wrapper">
          <div className="code-editor-line-numbers">
            {Array.from({ length: lineCount }, (_, i) => (
              <span key={i + 1} className="code-editor-line-number">
                {i + 1}
              </span>
            ))}
          </div>
          <textarea
            ref={textareaRef}
            className="code-editor-textarea"
            value={code}
            onChange={(e) => onChange(e.target.value)}
            onKeyDown={handleKeyDown}
            placeholder="// Enter your code here..."
            spellCheck={false}
            rows={Math.max(10, lineCount)}
          />
        </div>
        
        {/* Editor hints */}
        <div className="code-editor-hints">
          <CodeEditorHints language={language} />
        </div>
      </div>
    </CollapsibleSection>
  );
}

/**
 * Context hints for the code editor.
 */
function CodeEditorHints({ language }: { language: CodeLanguage }) {
  return (
    <div className="code-hint-box">
      <div className="code-hint-title">Available Context</div>
      <ul className="code-hint-list">
        <li><code>input</code> - Input data from state mapping</li>
        <li><code>return value;</code> - Return the result</li>
        {language === 'typescript' && (
          <li><code>interface</code> - Define types for better hints</li>
        )}
        <li>Standard {language === 'typescript' ? 'TypeScript' : 'JavaScript'} APIs available</li>
      </ul>
      <div className="code-hint-warning">
        ⚠️ Code runs in a sandboxed environment with configurable restrictions
      </div>
    </div>
  );
}

// ============================================
// Type Hints Section Component
// ============================================

interface TypeHintsSectionProps {
  inputType?: string;
  outputType?: string;
  onInputTypeChange: (type: string | undefined) => void;
  onOutputTypeChange: (type: string | undefined) => void;
}

/**
 * Type hints section for TypeScript type definitions.
 */
function TypeHintsSection({ 
  inputType, 
  outputType, 
  onInputTypeChange, 
  onOutputTypeChange 
}: TypeHintsSectionProps) {
  return (
    <CollapsibleSection title="Type Hints" defaultOpen={false}>
      <Field label="Input Type" hint="TypeScript type definition">
        <textarea
          className="code-panel-type-input"
          value={inputType || ''}
          onChange={(e) => onInputTypeChange(e.target.value || undefined)}
          placeholder="{ data: Array<{ id: string; name: string }> }"
          rows={3}
          spellCheck={false}
        />
      </Field>
      
      <Field label="Output Type" hint="TypeScript type definition">
        <textarea
          className="code-panel-type-input"
          value={outputType || ''}
          onChange={(e) => onOutputTypeChange(e.target.value || undefined)}
          placeholder="Array<{ id: string; name: string; processed: boolean }>"
          rows={3}
          spellCheck={false}
        />
      </Field>
      
      <div className="code-panel-info">
        <span className="code-panel-info-icon">ℹ️</span>
        <span className="code-panel-info-text">
          Type hints improve editor auto-completion and help catch errors early.
        </span>
      </div>
    </CollapsibleSection>
  );
}

// ============================================
// Sandbox Section Component
// ============================================

interface SandboxSectionProps {
  sandbox: SandboxConfig;
  onChange: (updates: Partial<SandboxConfig>) => void;
}

/**
 * Sandbox security configuration section.
 * @see Requirement 10.2
 */
function SandboxSection({ sandbox, onChange }: SandboxSectionProps) {
  // Calculate security level
  const getSecurityLevel = (): 'strict' | 'relaxed' | 'open' => {
    if (!sandbox.networkAccess && !sandbox.fileSystemAccess) return 'strict';
    if (sandbox.networkAccess && sandbox.fileSystemAccess) return 'open';
    return 'relaxed';
  };
  
  const securityLevel = getSecurityLevel();
  
  return (
    <CollapsibleSection title="Sandbox Security" defaultOpen={false}>
      {/* Security Level Indicator */}
      <div className={`code-sandbox-level code-sandbox-level-${securityLevel}`}>
        <span className="code-sandbox-level-icon">
          {securityLevel === 'strict' ? '🔒' : securityLevel === 'relaxed' ? '🔓' : '⚠️'}
        </span>
        <span className="code-sandbox-level-label">
          {securityLevel === 'strict' ? 'Strict Sandbox' : 
           securityLevel === 'relaxed' ? 'Relaxed Sandbox' : 'Open Access'}
        </span>
        <span className="code-sandbox-level-description">
          {securityLevel === 'strict' ? 'No network or file system access' : 
           securityLevel === 'relaxed' ? 'Limited access enabled' : 'Full access enabled'}
        </span>
      </div>
      
      {/* Access Toggles */}
      <div className="code-sandbox-toggles">
        <Field label="Network Access" hint="allow HTTP requests">
          <label className="code-panel-toggle">
            <input
              type="checkbox"
              checked={sandbox.networkAccess}
              onChange={(e) => onChange({ networkAccess: e.target.checked })}
            />
            <span className="code-panel-toggle-slider" />
            <span className="code-panel-toggle-label">
              {sandbox.networkAccess ? '🌐 Enabled' : '🚫 Disabled'}
            </span>
          </label>
        </Field>
        
        <Field label="File System Access" hint="allow file operations">
          <label className="code-panel-toggle">
            <input
              type="checkbox"
              checked={sandbox.fileSystemAccess}
              onChange={(e) => onChange({ fileSystemAccess: e.target.checked })}
            />
            <span className="code-panel-toggle-slider" />
            <span className="code-panel-toggle-label">
              {sandbox.fileSystemAccess ? '📁 Enabled' : '🚫 Disabled'}
            </span>
          </label>
        </Field>
      </div>
      
      {/* Resource Limits */}
      <div className="code-sandbox-limits">
        <Field label="Memory Limit" hint="MB">
          <div className="code-sandbox-limit-input">
            <input
              type="number"
              className="code-panel-input"
              value={sandbox.memoryLimit}
              onChange={(e) => onChange({ memoryLimit: parseInt(e.target.value, 10) || 0 })}
              min={0}
              max={1024}
              step={32}
            />
            <span className="code-sandbox-limit-unit">MB</span>
          </div>
          <div className="code-panel-field-help">
            Maximum memory allocation. Set to 0 for unlimited (not recommended).
          </div>
        </Field>
        
        <Field label="Time Limit" hint="ms">
          <div className="code-sandbox-limit-input">
            <input
              type="number"
              className="code-panel-input"
              value={sandbox.timeLimit}
              onChange={(e) => onChange({ timeLimit: parseInt(e.target.value, 10) || 0 })}
              min={0}
              max={60000}
              step={1000}
            />
            <span className="code-sandbox-limit-unit">ms</span>
          </div>
          <div className="code-panel-field-help">
            Maximum execution time. Set to 0 for unlimited (not recommended).
          </div>
        </Field>
      </div>
      
      {/* Security Warning */}
      {securityLevel !== 'strict' && (
        <div className="code-sandbox-warning">
          <span className="code-sandbox-warning-icon">⚠️</span>
          <span className="code-sandbox-warning-text">
            {securityLevel === 'open' 
              ? 'Full access enabled. Code can make network requests and access the file system. Use with caution.'
              : 'Some access restrictions have been relaxed. Ensure the code is trusted.'}
          </span>
        </div>
      )}
      
      {/* Reset to defaults */}
      <button
        type="button"
        className="code-sandbox-reset"
        onClick={() => onChange(DEFAULT_SANDBOX)}
      >
        Reset to Secure Defaults
      </button>
    </CollapsibleSection>
  );
}

export default CodePanel;
