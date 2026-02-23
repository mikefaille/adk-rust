import { describe, it, expect } from 'vitest';
import { generateTemplateWalkthroughSteps } from './useTemplateWalkthrough';
import type { Template } from '../components/Templates/templates';

describe('generateTemplateWalkthroughSteps', () => {
  const baseTemplate: Template = {
    id: 'test-template',
    name: 'Test Template',
    description: 'A test template',
    icon: '🧪',
    category: 'basic',
    agents: {},
    edges: [],
  };

  it('should generate basic steps for a minimal template', () => {
    const steps = generateTemplateWalkthroughSteps(baseTemplate);

    expect(steps).toHaveLength(3); // Overview, Docs, Ready

    // Overview Step
    expect(steps[0].id).toBe('overview');
    expect(steps[0].title).toBe('Welcome to Test Template');
    expect(steps[0].description).toBe('A test template');
    expect(steps[0].icon).toBe('🧪');

    // Docs Step
    expect(steps[1].id).toBe('docs');
    expect(steps[1].title).toBe('Learn More');

    // Ready Step
    expect(steps[2].id).toBe('ready');
    expect(steps[2].title).toBe('Ready to Go!');
  });

  it('should generate environment variable step when envVars are present', () => {
    const templateWithEnv: Template = {
      ...baseTemplate,
      envVars: [
        { name: 'API_KEY', description: 'An API Key', required: true },
        { name: 'OPTIONAL_VAR', description: 'Optional', required: false },
      ],
    };

    const steps = generateTemplateWalkthroughSteps(templateWithEnv);

    expect(steps).toHaveLength(4); // Overview, EnvVars, Docs, Ready

    const envStep = steps.find(s => s.id === 'env-vars');
    expect(envStep).toBeDefined();
    expect(envStep?.title).toBe('Configure Environment Variables');
    expect(envStep?.tips).toHaveLength(2);
    expect(envStep?.tips[0]).toContain('API_KEY (required)');
    expect(envStep?.tips[1]).toContain('Plus 1 optional variable(s)');
  });

  it('should generate agents step when agents are present', () => {
    const templateWithAgents: Template = {
      ...baseTemplate,
      agents: {
        agent1: {
          type: 'llm',
          model: 'gemini',
          instruction: 'test',
          tools: [],
          sub_agents: [],
          position: { x: 0, y: 0 }
        },
        agent2: {
          type: 'router',
          model: 'gemini',
          instruction: 'test',
          tools: [],
          sub_agents: [],
          position: { x: 0, y: 0 }
        }
      },
    };

    const steps = generateTemplateWalkthroughSteps(templateWithAgents);

    expect(steps).toHaveLength(4); // Overview, Agents, Docs, Ready

    const agentsStep = steps.find(s => s.id === 'agents');
    expect(agentsStep).toBeDefined();
    expect(agentsStep?.title).toBe('AI Agents in This Workflow');
    expect(agentsStep?.tips).toHaveLength(2);
    expect(agentsStep?.highlightNodes).toEqual(['agent1', 'agent2']);
  });

  it('should generate action nodes step when action nodes are present', () => {
    const templateWithActions: Template = {
      ...baseTemplate,
      actionNodes: {
        trigger1: {
          id: 'trigger1',
          type: 'trigger',
          name: 'Trigger',
          description: 'A trigger',
          triggerType: 'webhook',
          webhook: { path: '/', method: 'GET', auth: 'none' },
          errorHandling: { mode: 'stop' },
          tracing: { enabled: true, logLevel: 'info' },
          callbacks: {},
          execution: { timeout: 1000 },
          mapping: { outputKey: 'out' },
          position: { x: 0, y: 0 }
        },
        http1: {
          id: 'http1',
          type: 'http',
          name: 'HTTP Request',
          description: 'An HTTP request',
          method: 'GET',
          url: 'http://example.com',
          auth: { type: 'none' },
          headers: {},
          body: { type: 'none' },
          response: { type: 'json' },
          errorHandling: { mode: 'stop' },
          tracing: { enabled: true, logLevel: 'info' },
          callbacks: {},
          execution: { timeout: 1000 },
          mapping: { outputKey: 'out' },
          position: { x: 0, y: 0 }
        }
      },
    };

    const steps = generateTemplateWalkthroughSteps(templateWithActions);

    expect(steps).toHaveLength(4); // Overview, ActionNodes, Docs, Ready

    const actionStep = steps.find(s => s.id === 'action-nodes');
    expect(actionStep).toBeDefined();
    expect(actionStep?.title).toBe('Action Nodes for Automation');
    expect(actionStep?.tips).toHaveLength(2);
    expect(actionStep?.highlightNodes).toEqual(['trigger1', 'http1']);
  });

  it('should generate customization step when customization tips are present', () => {
    const templateWithTips: Template = {
      ...baseTemplate,
      customizationTips: ['Tip 1', 'Tip 2'],
    };

    const steps = generateTemplateWalkthroughSteps(templateWithTips);

    expect(steps).toHaveLength(4); // Overview, Customization, Docs, Ready

    const customStep = steps.find(s => s.id === 'customization');
    expect(customStep).toBeDefined();
    expect(customStep?.title).toBe('Customize for Your Needs');
    expect(customStep?.tips).toEqual(['Tip 1', 'Tip 2']);
  });

  it('should generate all steps for a complex template', () => {
    const complexTemplate: Template = {
      ...baseTemplate,
      envVars: [{ name: 'VAR', description: 'desc', required: true }],
      agents: { agent1: { type: 'llm', model: 'gemini', instruction: '', tools: [], sub_agents: [], position: { x: 0, y: 0 } } },
      actionNodes: {
        trigger1: {
          id: 'trigger1',
          type: 'trigger',
          name: 'Trigger',
          description: 'A trigger',
          triggerType: 'webhook',
          webhook: { path: '/', method: 'GET', auth: 'none' },
          errorHandling: { mode: 'stop' },
          tracing: { enabled: true, logLevel: 'info' },
          callbacks: {},
          execution: { timeout: 1000 },
          mapping: { outputKey: 'out' },
          position: { x: 0, y: 0 }
        }
      },
      customizationTips: ['Tip 1'],
    };

    const steps = generateTemplateWalkthroughSteps(complexTemplate);

    expect(steps).toHaveLength(7); // Overview, EnvVars, Agents, ActionNodes, Customization, Docs, Ready

    expect(steps.map(s => s.id)).toEqual([
      'overview',
      'env-vars',
      'agents',
      'action-nodes',
      'customization',
      'docs',
      'ready'
    ]);
  });
});
