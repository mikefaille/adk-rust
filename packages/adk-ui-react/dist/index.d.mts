import React from 'react';

type Component = {
    type: 'text';
    id?: string;
    content: string;
    variant?: TextVariant;
} | {
    type: 'button';
    id?: string;
    label: string;
    action_id: string;
    variant?: ButtonVariant;
    disabled?: boolean;
    icon?: string;
} | {
    type: 'icon';
    id?: string;
    name: string;
    size?: number;
} | {
    type: 'image';
    id?: string;
    src: string;
    alt?: string;
} | {
    type: 'badge';
    id?: string;
    label: string;
    variant?: BadgeVariant;
} | {
    type: 'text_input';
    id?: string;
    name: string;
    label: string;
    input_type?: 'text' | 'email' | 'password' | 'tel' | 'url';
    placeholder?: string;
    required?: boolean;
    default_value?: string;
    min_length?: number;
    max_length?: number;
    error?: string;
} | {
    type: 'number_input';
    id?: string;
    name: string;
    label: string;
    min?: number;
    max?: number;
    step?: number;
    required?: boolean;
    default_value?: number;
    error?: string;
} | {
    type: 'select';
    id?: string;
    name: string;
    label: string;
    options: SelectOption[];
    required?: boolean;
    error?: string;
} | {
    type: 'multi_select';
    id?: string;
    name: string;
    label: string;
    options: SelectOption[];
    required?: boolean;
} | {
    type: 'switch';
    id?: string;
    name: string;
    label: string;
    default_checked?: boolean;
} | {
    type: 'date_input';
    id?: string;
    name: string;
    label: string;
    required?: boolean;
} | {
    type: 'slider';
    id?: string;
    name: string;
    label: string;
    min?: number;
    max?: number;
    step?: number;
    default_value?: number;
} | {
    type: 'textarea';
    id?: string;
    name: string;
    label: string;
    placeholder?: string;
    rows?: number;
    required?: boolean;
    default_value?: string;
    error?: string;
} | {
    type: 'stack';
    id?: string;
    direction: 'horizontal' | 'vertical';
    children: Component[];
    gap?: number;
} | {
    type: 'grid';
    id?: string;
    columns: number;
    children: Component[];
    gap?: number;
} | {
    type: 'card';
    id?: string;
    title?: string;
    description?: string;
    content: Component[];
    footer?: Component[];
} | {
    type: 'container';
    id?: string;
    children: Component[];
    padding?: number;
} | {
    type: 'divider';
    id?: string;
} | {
    type: 'tabs';
    id?: string;
    tabs: Tab[];
} | {
    type: 'table';
    id?: string;
    columns: TableColumn[];
    data: Record<string, unknown>[];
    sortable?: boolean;
    page_size?: number;
    striped?: boolean;
} | {
    type: 'list';
    id?: string;
    items: string[];
    ordered?: boolean;
} | {
    type: 'key_value';
    id?: string;
    pairs: KeyValuePair[];
} | {
    type: 'code_block';
    id?: string;
    code: string;
    language?: string;
} | {
    type: 'chart';
    id?: string;
    title?: string;
    kind: ChartKind;
    data: Record<string, unknown>[];
    x_key: string;
    y_keys: string[];
    x_label?: string;
    y_label?: string;
    show_legend?: boolean;
    colors?: string[];
} | {
    type: 'alert';
    id?: string;
    title: string;
    description?: string;
    variant?: AlertVariant;
} | {
    type: 'progress';
    id?: string;
    value: number;
    label?: string;
} | {
    type: 'toast';
    id?: string;
    message: string;
    variant?: AlertVariant;
    duration?: number;
    dismissible?: boolean;
} | {
    type: 'modal';
    id?: string;
    title: string;
    content: Component[];
    footer?: Component[];
    size?: ModalSize;
    closable?: boolean;
} | {
    type: 'spinner';
    id?: string;
    size?: SpinnerSize;
    label?: string;
} | {
    type: 'skeleton';
    id?: string;
    variant?: SkeletonVariant;
    width?: string;
    height?: string;
};
type TextVariant = 'h1' | 'h2' | 'h3' | 'h4' | 'body' | 'caption' | 'code';
type ButtonVariant = 'primary' | 'secondary' | 'danger' | 'ghost' | 'outline';
type BadgeVariant = 'default' | 'info' | 'success' | 'warning' | 'error' | 'secondary' | 'outline';
type AlertVariant = 'info' | 'success' | 'warning' | 'error';
type ChartKind = 'bar' | 'line' | 'area' | 'pie';
type ModalSize = 'small' | 'medium' | 'large' | 'full';
type SpinnerSize = 'small' | 'medium' | 'large';
type SkeletonVariant = 'text' | 'circle' | 'rectangle';
interface SelectOption {
    label: string;
    value: string;
}
interface TableColumn {
    header: string;
    accessor_key: string;
    sortable?: boolean;
}
interface Tab {
    label: string;
    content: Component[];
}
interface KeyValuePair {
    key: string;
    value: string;
}
interface UiResponse {
    id?: string;
    theme?: 'light' | 'dark' | 'system';
    components: Component[];
}
type UiEvent = {
    action: 'form_submit';
    action_id: string;
    data: Record<string, unknown>;
} | {
    action: 'button_click';
    action_id: string;
} | {
    action: 'input_change';
    name: string;
    value: unknown;
} | {
    action: 'tab_change';
    index: number;
};
declare function uiEventToMessage(event: UiEvent): string;

interface RendererProps {
    component: Component;
    onAction?: (event: UiEvent) => void;
    /** Theme for this component: 'dark' wraps in dark mode styling */
    theme?: 'light' | 'dark' | 'system';
}
declare const Renderer: React.FC<RendererProps>;

export { type Component, Renderer, type TableColumn, type UiEvent, type UiResponse, uiEventToMessage };
