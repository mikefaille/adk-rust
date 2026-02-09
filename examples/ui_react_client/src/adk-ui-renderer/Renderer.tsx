import React, { createContext, useContext, useRef, useState, useCallback, useMemo } from 'react';
import type { Component, UiEvent } from './types';
import { AlertCircle, CheckCircle, Info, XCircle, User, Mail, Calendar } from 'lucide-react';
import Markdown from 'react-markdown';
import clsx from 'clsx';
import {
    BarChart, LineChart, AreaChart, PieChart,
    Bar, Line, Area, Pie, Cell,
    XAxis, YAxis, CartesianGrid, Tooltip, Legend, ResponsiveContainer
} from 'recharts';

/**
 * --- Constants & Configuration ---
 */

const IconMap: Record<string, React.ComponentType<any>> = {
    'alert-circle': AlertCircle,
    'check-circle': CheckCircle,
    'info': Info,
    'x-circle': XCircle,
    'user': User,
    'mail': Mail,
    'calendar': Calendar,
};

const DEFAULT_CHART_COLORS = ['#3B82F6', '#10B981', '#F59E0B', '#EF4444', '#8B5CF6', '#EC4899', '#06B6D4'];

/**
 * --- Shared Types ---
 */

interface FormContextValue {
    onAction?: (event: UiEvent) => void;
}

const FormContext = createContext<FormContextValue>({});

type InputComponent = Extract<Component, {
    type: 'text_input' | 'number_input' | 'select' | 'multi_select' | 'switch' | 'date_input' | 'slider' | 'textarea'
}>;

export interface RendererProps {
    component: Component;
    onAction?: (event: UiEvent) => void;
    /** Theme for this component: 'dark' wraps in dark mode styling */
    theme?: 'light' | 'dark' | 'system';
}

/**
 * --- Formatting Helpers ---
 */

interface PieLabelProps {
    name: string;
    percent?: number;
}

/**
 * Pure function for rendering Pie chart labels.
 * Extracted to a constant to stabilize prop references.
 */
const renderPieLabel = ({ name, percent }: PieLabelProps) =>
    `${name}: ${((percent ?? 0) * 100).toFixed(0)}%`;

/**
 * --- Main Entry Point ---
 */

/**
 * High-level Renderer component that provides theming and action handling context.
 */
export function Renderer({ component, onAction, theme }: RendererProps) {
    const isDark = theme === 'dark';
    const contextValue = useMemo(() => ({ onAction }), [onAction]);

    return (
        <FormContext.Provider value={contextValue}>
            <div className={isDark ? 'dark' : ''}>
                <ComponentRenderer component={component} />
            </div>
        </FormContext.Provider>
    );
}

/**
 * Internal recursive renderer that maps component types to their specialized renderer components.
 */
function ComponentRenderer({ component }: { component: Component }) {
    const { onAction } = useContext(FormContext);
    const formRef = useRef<HTMLFormElement>(null);

    const handleButtonClick = useCallback((actionId: string) => {
        if (formRef.current) {
            const formData = new FormData(formRef.current);
            const data: Record<string, unknown> = {};
            formData.forEach((value, key) => {
                data[key] = value;
            });
            onAction?.({ action: 'form_submit', action_id: actionId, data });
        } else {
            onAction?.({ action: 'button_click', action_id: actionId });
        }
    }, [onAction]);

    const handleToastDismiss = useCallback(() => {
        onAction?.({ action: 'button_click', action_id: 'toast_dismiss' });
    }, [onAction]);

    const handleModalClose = useCallback(() => {
        onAction?.({ action: 'button_click', action_id: 'modal_close' });
    }, [onAction]);

    switch (component.type) {
        case 'text':
            return <TextRenderer component={component} />;

        case 'button':
            return <ButtonRenderer component={component} onClick={handleButtonClick} />;

        case 'icon': {
            const Icon = IconMap[component.name] || Info;
            return <Icon size={component.size || 24} />;
        }

        case 'alert':
            return <AlertRenderer component={component} />;

        case 'card':
            return <CardRenderer component={component} formRef={formRef} onAction={onAction} />;

        case 'stack':
            return <StackRenderer component={component} />;

        case 'text_input':
        case 'number_input':
        case 'select':
        case 'switch':
        case 'multi_select':
        case 'date_input':
        case 'slider':
        case 'textarea':
            return <InputRenderer component={component} />;

        case 'progress':
            return <ProgressRenderer component={component} />;

        case 'spinner':
            return <SpinnerRenderer component={component} />;

        case 'skeleton':
            return <SkeletonRenderer component={component} />;

        case 'toast':
            return <ToastRenderer component={component} onDismiss={handleToastDismiss} />;

        case 'modal':
            return <ModalRenderer component={component} onClose={handleModalClose} />;

        case 'grid':
            return <GridRenderer component={component} />;

        case 'list':
            return <ListRenderer component={component} />;

        case 'key_value':
            return <KeyValueRenderer component={component} />;

        case 'tabs':
            return <TabsRenderer component={component} />;

        case 'table':
            return <TableRenderer component={component} />;

        case 'chart':
            return <ChartRenderer component={component} />;

        case 'code_block':
            return <CodeBlockRenderer component={component} />;

        case 'image':
            return <ImageRenderer component={component} />;

        case 'badge':
            return <BadgeRenderer component={component} />;

        case 'divider':
            return <hr className="my-4 border-gray-200" />;

        case 'container':
            return <ContainerRenderer component={component} />;

        default:
            return <div className="text-red-500 text-sm p-2 border border-red-200 rounded">Unknown component: {(component as any).type}</div>;
    }
}

/**
 * --- Specialized Sub-Renderers ---
 */

function TextRenderer({ component }: { component: Extract<Component, { type: 'text' }> }) {
    if (component.variant === 'body' || !component.variant) {
        return (
            <div className="prose prose-sm dark:prose-invert max-w-none text-gray-700 dark:text-gray-300">
                <Markdown>{component.content}</Markdown>
            </div>
        );
    }
    const Tag = component.variant === 'h1' ? 'h1' :
        component.variant === 'h2' ? 'h2' :
            component.variant === 'h3' ? 'h3' :
                component.variant === 'h4' ? 'h4' :
                    component.variant === 'code' ? 'code' : 'p';
    const classes = clsx({
        'text-4xl font-bold mb-4 dark:text-white': component.variant === 'h1',
        'text-3xl font-bold mb-3 dark:text-white': component.variant === 'h2',
        'text-2xl font-bold mb-2 dark:text-white': component.variant === 'h3',
        'text-xl font-bold mb-2 dark:text-white': component.variant === 'h4',
        'font-mono bg-gray-100 dark:bg-gray-800 p-1 rounded dark:text-gray-100': component.variant === 'code',
        'text-sm text-gray-500 dark:text-gray-400': component.variant === 'caption',
    });
    return <Tag className={classes}>{component.content}</Tag>;
}

function ButtonRenderer({ component, onClick }: {
    component: Extract<Component, { type: 'button' }>;
    onClick: (actionId: string) => void;
}) {
    const btnClasses = clsx('px-4 py-2 rounded font-medium transition-colors', {
        'bg-blue-600 text-white hover:bg-blue-700': component.variant === 'primary' || !component.variant,
        'bg-gray-200 text-gray-800 hover:bg-gray-300': component.variant === 'secondary',
        'bg-red-600 text-white hover:bg-red-700': component.variant === 'danger',
        'bg-transparent hover:bg-gray-100': component.variant === 'ghost',
        'border border-gray-300 hover:bg-gray-50': component.variant === 'outline',
        'opacity-50 cursor-not-allowed': component.disabled,
    });
    return (
        <button
            type="button"
            className={btnClasses}
            disabled={component.disabled}
            onClick={() => onClick(component.action_id)}
        >
            {component.label}
        </button>
    );
}

function AlertRenderer({ component }: { component: Extract<Component, { type: 'alert' }> }) {
    const alertClasses = clsx('p-4 rounded-md border mb-4 flex items-start gap-3', {
        'bg-blue-50 border-blue-200 text-blue-800': component.variant === 'info' || !component.variant,
        'bg-green-50 border-green-200 text-green-800': component.variant === 'success',
        'bg-yellow-50 border-yellow-200 text-yellow-800': component.variant === 'warning',
        'bg-red-50 border-red-200 text-red-800': component.variant === 'error',
    });
    const AlertIcon = component.variant === 'success' ? CheckCircle :
        component.variant === 'warning' ? AlertCircle :
            component.variant === 'error' ? XCircle : Info;
    return (
        <div className={alertClasses}>
            <AlertIcon className="w-5 h-5 mt-0.5" />
            <div>
                <div className="font-semibold">{component.title}</div>
                {component.description && <div className="text-sm mt-1 opacity-90">{component.description}</div>}
            </div>
        </div>
    );
}

function ChartRenderer({ component }: { component: Extract<Component, { type: 'chart' }> }) {
    const chartColors = component.colors || DEFAULT_CHART_COLORS;
    const chartKind = component.kind || 'bar';
    const showLegend = component.show_legend !== false;

    return (
        <div className="mb-4 p-4 bg-white dark:bg-gray-900 border dark:border-gray-700 rounded-lg">
            {component.title && <h4 className="font-semibold text-lg mb-4 dark:text-white">{component.title}</h4>}
            <ResponsiveContainer width="100%" height={300}>
                {chartKind === 'line' ? (
                    <LineChart data={component.data}>
                        <CartesianGrid strokeDasharray="3 3" />
                        <XAxis dataKey={component.x_key} label={component.x_label ? { value: component.x_label, position: 'bottom' } : undefined} />
                        <YAxis label={component.y_label ? { value: component.y_label, angle: -90, position: 'insideLeft' } : undefined} />
                        <Tooltip />
                        {showLegend && <Legend />}
                        {component.y_keys.map((key, i) => (
                            <Line key={key} type="monotone" dataKey={key} stroke={chartColors[i % chartColors.length]} strokeWidth={2} />
                        ))}
                    </LineChart>
                ) : chartKind === 'area' ? (
                    <AreaChart data={component.data}>
                        <CartesianGrid strokeDasharray="3 3" />
                        <XAxis dataKey={component.x_key} label={component.x_label ? { value: component.x_label, position: 'bottom' } : undefined} />
                        <YAxis label={component.y_label ? { value: component.y_label, angle: -90, position: 'insideLeft' } : undefined} />
                        <Tooltip />
                        {showLegend && <Legend />}
                        {component.y_keys.map((key, i) => (
                            <Area key={key} type="monotone" dataKey={key} fill={chartColors[i % chartColors.length]} fillOpacity={0.6} stroke={chartColors[i % chartColors.length]} />
                        ))}
                    </AreaChart>
                ) : chartKind === 'pie' ? (
                    <PieChart>
                        <Pie
                            data={component.data}
                            dataKey={component.y_keys[0]}
                            nameKey={component.x_key}
                            cx="50%"
                            cy="50%"
                            outerRadius={100}
                            label={renderPieLabel}
                        >
                            {component.data.map((_, i) => (
                                <Cell key={i} fill={chartColors[i % chartColors.length]} />
                            ))}
                        </Pie>
                        <Tooltip />
                        {showLegend && <Legend />}
                    </PieChart>
                ) : (
                    <BarChart data={component.data}>
                        <CartesianGrid strokeDasharray="3 3" />
                        <XAxis dataKey={component.x_key} label={component.x_label ? { value: component.x_label, position: 'bottom' } : undefined} />
                        <YAxis label={component.y_label ? { value: component.y_label, angle: -90, position: 'insideLeft' } : undefined} />
                        <Tooltip />
                        {showLegend && <Legend />}
                        {component.y_keys.map((key, i) => (
                            <Bar key={key} dataKey={key} fill={chartColors[i % chartColors.length]} />
                        ))}
                    </BarChart>
                )}
            </ResponsiveContainer>
        </div>
    );
}

function ImageRenderer({ component }: { component: Extract<Component, { type: 'image' }> }) {
    return (
        <div className="mb-4">
            <img
                src={component.src}
                alt={component.alt || ''}
                className="max-w-full h-auto rounded-lg"
            />
        </div>
    );
}

function BadgeRenderer({ component }: { component: Extract<Component, { type: 'badge' }> }) {
    const badgeClasses = clsx('inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium', {
        'bg-gray-100 text-gray-800': component.variant === 'default' || !component.variant,
        'bg-blue-100 text-blue-800': component.variant === 'info',
        'bg-green-100 text-green-800': component.variant === 'success',
        'bg-yellow-100 text-yellow-800': component.variant === 'warning',
        'bg-red-100 text-red-800': component.variant === 'error',
        'bg-gray-200 text-gray-700': component.variant === 'secondary',
        'bg-transparent border border-gray-300 text-gray-700': component.variant === 'outline',
    });
    return <span className={badgeClasses}>{component.label}</span>;
}

function CodeBlockRenderer({ component }: { component: Extract<Component, { type: 'code_block' }> }) {
    return (
        <div className="mb-4">
            <pre className="bg-gray-900 text-gray-100 p-4 rounded-lg overflow-x-auto text-sm">
                <code>{component.code}</code>
            </pre>
        </div>
    );
}

function InputRenderer({ component }: { component: InputComponent }) {
    switch (component.type) {
        case 'text_input': {
            const inputType = component.input_type || 'text';
            return (
                <div className="mb-3">
                    <label className="block text-sm font-medium text-gray-700 dark:text-gray-300 mb-1">{component.label}</label>
                    <input
                        type={inputType}
                        name={component.name}
                        placeholder={component.placeholder}
                        defaultValue={component.default_value}
                        required={component.required}
                        className={clsx('w-full px-3 py-2 border rounded-md focus:ring-2 focus:ring-blue-500 focus:border-blue-500 outline-none bg-white dark:bg-gray-800 dark:border-gray-600 dark:text-white', {
                            'border-red-500 focus:ring-red-500 focus:border-red-500': component.error,
                        })}
                    />
                    {component.error && (
                        <p className="text-red-500 dark:text-red-400 text-sm mt-1">{component.error}</p>
                    )}
                </div>
            );
        }
        case 'number_input':
            return (
                <div className="mb-3">
                    <label className="block text-sm font-medium text-gray-700 mb-1">{component.label}</label>
                    <input
                        type="number"
                        name={component.name}
                        min={component.min}
                        max={component.max}
                        step={component.step}
                        required={component.required}
                        className={clsx('w-full px-3 py-2 border rounded-md focus:ring-2 focus:ring-blue-500 focus:border-blue-500 outline-none', {
                            'border-red-500 focus:ring-red-500 focus:border-red-500': component.error,
                        })}
                    />
                    {component.error && (
                        <p className="text-red-500 text-sm mt-1">{component.error}</p>
                    )}
                </div>
            );
        case 'select':
            return (
                <div className="mb-3">
                    <label className="block text-sm font-medium text-gray-700 mb-1">{component.label}</label>
                    <select
                        name={component.name}
                        required={component.required}
                        className={clsx('w-full px-3 py-2 border rounded-md focus:ring-2 focus:ring-blue-500 focus:border-blue-500 outline-none', {
                            'border-red-500 focus:ring-red-500 focus:border-red-500': component.error,
                        })}
                    >
                        <option value="">Select...</option>
                        {component.options.map((opt, i) => (
                            <option key={i} value={opt.value}>{opt.label}</option>
                        ))}
                    </select>
                    {component.error && (
                        <p className="text-red-500 text-sm mt-1">{component.error}</p>
                    )}
                </div>
            );
        case 'switch':
            return (
                <div className="mb-3 flex items-center">
                    <input
                        type="checkbox"
                        name={component.name}
                        defaultChecked={component.default_checked}
                        className="h-4 w-4 rounded border-gray-300 text-blue-600 focus:ring-blue-500"
                    />
                    <label className="ml-2 text-sm font-medium text-gray-700">{component.label}</label>
                </div>
            );
        case 'multi_select':
            return (
                <div className="mb-3">
                    <label className="block text-sm font-medium text-gray-700 mb-1">{component.label}</label>
                    <select
                        name={component.name}
                        multiple
                        required={component.required}
                        size={Math.min(component.options.length, 5)}
                        className="w-full px-3 py-2 border rounded-md focus:ring-2 focus:ring-blue-500 focus:border-blue-500 outline-none"
                    >
                        {component.options.map((opt, i) => (
                            <option key={i} value={opt.value}>{opt.label}</option>
                        ))}
                    </select>
                </div>
            );
        case 'date_input':
            return (
                <div className="mb-3">
                    <label className="block text-sm font-medium text-gray-700 mb-1">{component.label}</label>
                    <input
                        type="date"
                        name={component.name}
                        required={component.required}
                        className="w-full px-3 py-2 border rounded-md focus:ring-2 focus:ring-blue-500 focus:border-blue-500 outline-none"
                    />
                </div>
            );
        case 'slider':
            return (
                <div className="mb-3">
                    <label className="block text-sm font-medium text-gray-700 mb-1">{component.label}</label>
                    <input
                        type="range"
                        name={component.name}
                        min={component.min}
                        max={component.max}
                        step={component.step}
                        defaultValue={component.default_value}
                        className="w-full h-2 bg-gray-200 rounded-lg appearance-none cursor-pointer"
                    />
                </div>
            );
        case 'textarea':
            return (
                <div className="mb-3">
                    <label className="block text-sm font-medium text-gray-700 dark:text-gray-300 mb-1">{component.label}</label>
                    <textarea
                        name={component.name}
                        placeholder={component.placeholder}
                        rows={component.rows || 4}
                        required={component.required}
                        defaultValue={component.default_value}
                        className={clsx('w-full px-3 py-2 border rounded-md focus:ring-2 focus:ring-blue-500 focus:border-blue-500 outline-none bg-white dark:bg-gray-800 dark:border-gray-600 dark:text-white resize-y', {
                            'border-red-500 focus:ring-red-500 focus:border-red-500': component.error,
                        })}
                    />
                    {component.error && (
                        <p className="text-red-500 dark:text-red-400 text-sm mt-1">{component.error}</p>
                    )}
                </div>
            );
        default:
            return null;
    }
}

function TableRenderer({ component }: { component: Extract<Component, { type: 'table' }> }) {
    const [sortColumn, setSortColumn] = useState<string | null>(null);
    const [sortDirection, setSortDirection] = useState<'asc' | 'desc'>('asc');
    const [currentPage, setCurrentPage] = useState(0);

    const handleSort = useCallback((accessorKey: string) => {
        if (!component.sortable) return;
        if (sortColumn === accessorKey) {
            setSortDirection(prev => (prev === 'asc' ? 'desc' : 'asc'));
        } else {
            setSortColumn(accessorKey);
            setSortDirection('asc');
        }
    }, [component.sortable, sortColumn]);

    const tableData = useMemo(() => {
        const data = [...component.data];
        if (sortColumn) {
            data.sort((a, b) => {
                const aVal = a[sortColumn] ?? '';
                const bVal = b[sortColumn] ?? '';
                const cmp = String(aVal).localeCompare(String(bVal));
                return sortDirection === 'asc' ? cmp : -cmp;
            });
        }
        return data;
    }, [component.data, sortColumn, sortDirection]);

    const pageSize = component.page_size || tableData.length;
    const totalPages = Math.ceil(tableData.length / pageSize);

    const paginatedData = useMemo(() => {
        return tableData.slice(currentPage * pageSize, (currentPage + 1) * pageSize);
    }, [tableData, currentPage, pageSize]);

    const handlePrevPage = useCallback(() => setCurrentPage(prev => Math.max(0, prev - 1)), []);
    const handleNextPage = useCallback(() => setCurrentPage(prev => Math.min(totalPages - 1, prev + 1)), [totalPages]);

    return (
        <div className="mb-4 overflow-x-auto">
            <table className={clsx('min-w-full divide-y divide-gray-200 dark:divide-gray-700 border dark:border-gray-700 rounded-lg overflow-hidden')}>
                <thead className="bg-gray-50 dark:bg-gray-800">
                    <tr>
                        {component.columns.map((col, i) => (
                            <th
                                key={i}
                                onClick={() => handleSort(col.accessor_key)}
                                className={clsx(
                                    'px-4 py-3 text-left text-xs font-medium text-gray-500 dark:text-gray-400 uppercase tracking-wider',
                                    component.sortable && col.sortable !== false && 'cursor-pointer hover:bg-gray-100 dark:hover:bg-gray-700'
                                )}
                            >
                                {col.header}
                                {sortColumn === col.accessor_key && (
                                    <span className="ml-1">{sortDirection === 'asc' ? '↑' : '↓'}</span>
                                )}
                            </th>
                        ))}
                    </tr>
                </thead>
                <tbody className="bg-white dark:bg-gray-900 divide-y divide-gray-200 dark:divide-gray-700">
                    {paginatedData.map((row, ri) => (
                        <tr key={ri} className={clsx(
                            'hover:bg-gray-50 dark:hover:bg-gray-800',
                            component.striped && ri % 2 === 1 && 'bg-gray-50 dark:bg-gray-800/50'
                        )}>
                            {component.columns.map((col, ci) => (
                                <td key={ci} className="px-4 py-3 text-sm text-gray-700 dark:text-gray-300">
                                    {String(row[col.accessor_key] ?? '')}
                                </td>
                            ))}
                        </tr>
                    ))}
                </tbody>
            </table>
            {component.page_size && totalPages > 1 && (
                <div className="flex items-center justify-between mt-2 px-2">
                    <span className="text-sm text-gray-500 dark:text-gray-400">
                        Page {currentPage + 1} of {totalPages}
                    </span>
                    <div className="flex gap-2">
                        <button
                            onClick={handlePrevPage}
                            disabled={currentPage === 0}
                            className="px-3 py-1 text-sm border rounded hover:bg-gray-100 dark:hover:bg-gray-700 disabled:opacity-50 dark:border-gray-600 dark:text-gray-300"
                        >
                            Previous
                        </button>
                        <button
                            onClick={handleNextPage}
                            disabled={currentPage === totalPages - 1}
                            className="px-3 py-1 text-sm border rounded hover:bg-gray-100 dark:hover:bg-gray-700 disabled:opacity-50 dark:border-gray-600 dark:text-gray-300"
                        >
                            Next
                        </button>
                    </div>
                </div>
            )}
        </div>
    );
}

function TabsRenderer({ component }: { component: Extract<Component, { type: 'tabs' }> }) {
    const [activeTab, setActiveTab] = useState(0);

    const handleTabChange = useCallback((index: number) => {
        setActiveTab(index);
    }, []);

    return (
        <div className="mb-4">
            <div className="border-b border-gray-200">
                <nav className="flex space-x-4">
                    {component.tabs.map((tab, i) => (
                        <button
                            key={i}
                            onClick={() => handleTabChange(i)}
                            className={clsx('px-4 py-2 border-b-2 font-medium text-sm transition-colors', {
                                'border-blue-600 text-blue-600': activeTab === i,
                                'border-transparent text-gray-500 hover:text-gray-700': activeTab !== i,
                            })}
                        >
                            {tab.label}
                        </button>
                    ))}
                </nav>
            </div>
            <div className="p-4">
                {component.tabs[activeTab].content.map((child, i) =>
                    <ComponentRenderer key={i} component={child} />
                )}
            </div>
        </div>
    );
}

function SpinnerRenderer({ component }: { component: Extract<Component, { type: 'spinner' }> }) {
    const spinnerSizes = {
        small: 'w-4 h-4',
        medium: 'w-8 h-8',
        large: 'w-12 h-12',
    };
    return (
        <div className="flex items-center gap-2">
            <div className={clsx('animate-spin rounded-full border-2 border-blue-600 border-t-transparent', spinnerSizes[component.size || 'medium'])} />
            {component.label && <span className="text-gray-600 dark:text-gray-400">{component.label}</span>}
        </div>
    );
}

function SkeletonRenderer({ component }: { component: Extract<Component, { type: 'skeleton' }> }) {
    return (
        <div
            className={clsx('animate-pulse bg-gray-200 dark:bg-gray-700', {
                'h-4 rounded': component.variant === 'text' || !component.variant,
                'rounded-full aspect-square': component.variant === 'circle',
                'rounded': component.variant === 'rectangle',
            })}
            style={{ width: component.width || '100%', height: component.height }}
        />
    );
}

function GridRenderer({ component }: { component: Extract<Component, { type: 'grid' }> }) {
    return (
        <div
            className="grid gap-4 mb-4"
            style={{ gridTemplateColumns: `repeat(${component.columns || 2}, 1fr)` }}
        >
            {component.children.map((child, i) => <ComponentRenderer key={i} component={child} />)}
        </div>
    );
}

function ListRenderer({ component }: { component: Extract<Component, { type: 'list' }> }) {
    return (
        <ul className="space-y-2 mb-4 list-disc list-inside">
            {component.items.map((item, i) => (
                <li key={i} className="text-gray-700">{item}</li>
            ))}
        </ul>
    );
}

function KeyValueRenderer({ component }: { component: Extract<Component, { type: 'key_value' }> }) {
    return (
        <dl className="grid grid-cols-2 gap-x-4 gap-y-2 mb-4">
            {component.pairs.map((pair, i) => (
                <React.Fragment key={i}>
                    <dt className="font-medium text-gray-700">{pair.key}:</dt>
                    <dd className="text-gray-900">{pair.value}</dd>
                </React.Fragment>
            ))}
        </dl>
    );
}

function ProgressRenderer({ component }: { component: Extract<Component, { type: 'progress' }> }) {
    return (
        <div className="mb-3">
            {component.label && <div className="text-sm text-gray-600 mb-1">{component.label}</div>}
            <div className="w-full bg-gray-200 rounded-full h-2.5">
                <div
                    className="bg-blue-600 h-2.5 rounded-full transition-all"
                    style={{ width: `${component.value}%` }}
                />
            </div>
        </div>
    );
}

function CardRenderer({ component, formRef, onAction }: {
    component: Extract<Component, { type: 'card' }>;
    formRef: React.RefObject<HTMLFormElement>;
    onAction?: (event: UiEvent) => void;
}) {
    const hasInputs = component.content.some(c =>
        c.type === 'text_input' || c.type === 'number_input' || c.type === 'select' || c.type === 'switch' || c.type === 'textarea'
    );

    const handleSubmit = (e: React.FormEvent<HTMLFormElement>) => {
        e.preventDefault();
        const formData = new FormData(e.currentTarget);
        const data: Record<string, unknown> = {};
        formData.forEach((value, key) => {
            data[key] = value;
        });
        const submitBtn = [...component.content, ...(component.footer || [])].find(
            c => c.type === 'button'
        ) as { type: 'button'; action_id: string } | undefined;
        onAction?.({
            action: 'form_submit',
            action_id: submitBtn?.action_id || 'form_submit',
            data
        });
    };

    const cardContent = (
        <>
            {(component.title || component.description) && (
                <div className="p-4 border-b dark:border-gray-700 bg-gray-50 dark:bg-gray-800">
                    {component.title && <h3 className="font-semibold text-lg dark:text-white">{component.title}</h3>}
                    {component.description && <p className="text-gray-500 dark:text-gray-400 text-sm">{component.description}</p>}
                </div>
            )}
            <div className="p-4">
                {component.content.map((child, i) => <ComponentRenderer key={i} component={child} />)}
            </div>
            {component.footer && (
                <div className="p-4 border-t dark:border-gray-700 bg-gray-50 dark:bg-gray-800 flex gap-2 justify-end">
                    {component.footer.map((child, i) => <ComponentRenderer key={i} component={child} />)}
                </div>
            )}
        </>
    );

    return hasInputs ? (
        <form ref={formRef} onSubmit={handleSubmit} className="bg-white dark:bg-gray-900 rounded-lg border dark:border-gray-700 shadow-sm overflow-hidden mb-4">
            {cardContent}
        </form>
    ) : (
        <div className="bg-white dark:bg-gray-900 rounded-lg border dark:border-gray-700 shadow-sm overflow-hidden mb-4">
            {cardContent}
        </div>
    );
}

function StackRenderer({ component }: { component: Extract<Component, { type: 'stack' }> }) {
    const stackClasses = clsx('flex', {
        'flex-col': component.direction === 'vertical',
        'flex-row': component.direction === 'horizontal',
    });
    return (
        <div className={stackClasses} style={{ gap: (component.gap || 4) * 4 }}>
            {component.children.map((child, i) => <ComponentRenderer key={i} component={child} />)}
        </div>
    );
}

function ToastRenderer({ component, onDismiss }: {
    component: Extract<Component, { type: 'toast' }>;
    onDismiss: () => void;
}) {
    const toastClasses = clsx('fixed bottom-4 right-4 p-4 rounded-lg shadow-lg flex items-center gap-3 z-50', {
        'bg-blue-50 border border-blue-200 text-blue-800': component.variant === 'info' || !component.variant,
        'bg-green-50 border border-green-200 text-green-800': component.variant === 'success',
        'bg-yellow-50 border border-yellow-200 text-yellow-800': component.variant === 'warning',
        'bg-red-50 border border-red-200 text-red-800': component.variant === 'error',
    });
    const ToastIcon = component.variant === 'success' ? CheckCircle :
        component.variant === 'warning' ? AlertCircle :
            component.variant === 'error' ? XCircle : Info;
    return (
        <div className={toastClasses}>
            <ToastIcon className="w-5 h-5" />
            <span>{component.message}</span>
            {component.dismissible !== false && (
                <button
                    onClick={onDismiss}
                    className="ml-2 text-gray-500 hover:text-gray-700"
                >
                    <XCircle className="w-4 h-4" />
                </button>
            )}
        </div>
    );
}

function ModalRenderer({ component, onClose }: {
    component: Extract<Component, { type: 'modal' }>;
    onClose: () => void;
}) {
    const modalSizes = {
        small: 'max-w-sm',
        medium: 'max-w-lg',
        large: 'max-w-2xl',
        full: 'max-w-full mx-4',
    };
    return (
        <div className="fixed inset-0 bg-black/50 flex items-center justify-center z-50">
            <div className={clsx('bg-white dark:bg-gray-900 rounded-lg shadow-xl w-full', modalSizes[component.size || 'medium'])}>
                <div className="p-4 border-b dark:border-gray-700 flex justify-between items-center">
                    <h3 className="font-semibold text-lg dark:text-white">{component.title}</h3>
                    {component.closable !== false && (
                        <button
                            onClick={onClose}
                            className="text-gray-500 hover:text-gray-700 dark:text-gray-400 dark:hover:text-gray-200"
                        >
                            <XCircle className="w-5 h-5" />
                        </button>
                    )}
                </div>
                <div className="p-4">
                    {component.content.map((child, i) => <ComponentRenderer key={i} component={child} />)}
                </div>
                {component.footer && (
                    <div className="p-4 border-t dark:border-gray-700 flex justify-end gap-2">
                        {component.footer.map((child, i) => <ComponentRenderer key={i} component={child} />)}
                    </div>
                )}
            </div>
        </div>
    );
}

function ContainerRenderer({ component }: { component: Extract<Component, { type: 'container' }> }) {
    return (
        <div className="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8">
            {component.children.map((child, i) => <ComponentRenderer key={i} component={child} />)}
        </div>
    );
}
