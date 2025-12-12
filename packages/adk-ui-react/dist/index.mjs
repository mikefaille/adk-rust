// src/Renderer.tsx
import React, { createContext, useContext, useRef } from "react";
import { AlertCircle, CheckCircle, Info, XCircle, User, Mail, Calendar } from "lucide-react";
import Markdown from "react-markdown";
import clsx from "clsx";
import {
  BarChart,
  LineChart,
  AreaChart,
  PieChart,
  Bar,
  Line,
  Area,
  Pie,
  Cell,
  XAxis,
  YAxis,
  CartesianGrid,
  Tooltip,
  Legend,
  ResponsiveContainer
} from "recharts";
import { Fragment, jsx, jsxs } from "react/jsx-runtime";
var IconMap = {
  "alert-circle": AlertCircle,
  "check-circle": CheckCircle,
  "info": Info,
  "x-circle": XCircle,
  "user": User,
  "mail": Mail,
  "calendar": Calendar
};
var FormContext = createContext({});
var Renderer = ({ component, onAction, theme }) => {
  const isDark = theme === "dark";
  return /* @__PURE__ */ jsx(FormContext.Provider, { value: { onAction }, children: /* @__PURE__ */ jsx("div", { className: isDark ? "dark" : "", children: /* @__PURE__ */ jsx(ComponentRenderer, { component }) }) });
};
var ComponentRenderer = ({ component }) => {
  const { onAction } = useContext(FormContext);
  const formRef = useRef(null);
  const handleButtonClick = (actionId) => {
    if (formRef.current) {
      const formData = new FormData(formRef.current);
      const data = {};
      formData.forEach((value, key) => {
        data[key] = value;
      });
      onAction?.({ action: "form_submit", action_id: actionId, data });
    } else {
      onAction?.({ action: "button_click", action_id: actionId });
    }
  };
  switch (component.type) {
    case "text":
      if (component.variant === "body" || !component.variant) {
        return /* @__PURE__ */ jsx("div", { className: "prose prose-sm dark:prose-invert max-w-none text-gray-700 dark:text-gray-300", children: /* @__PURE__ */ jsx(Markdown, { children: component.content }) });
      }
      const Tag = component.variant === "h1" ? "h1" : component.variant === "h2" ? "h2" : component.variant === "h3" ? "h3" : component.variant === "h4" ? "h4" : component.variant === "code" ? "code" : "p";
      const classes = clsx({
        "text-4xl font-bold mb-4 dark:text-white": component.variant === "h1",
        "text-3xl font-bold mb-3 dark:text-white": component.variant === "h2",
        "text-2xl font-bold mb-2 dark:text-white": component.variant === "h3",
        "text-xl font-bold mb-2 dark:text-white": component.variant === "h4",
        "font-mono bg-gray-100 dark:bg-gray-800 p-1 rounded dark:text-gray-100": component.variant === "code",
        "text-sm text-gray-500 dark:text-gray-400": component.variant === "caption"
      });
      return /* @__PURE__ */ jsx(Tag, { className: classes, children: component.content });
    case "button":
      const btnClasses = clsx("px-4 py-2 rounded font-medium transition-colors", {
        "bg-blue-600 text-white hover:bg-blue-700": component.variant === "primary" || !component.variant,
        "bg-gray-200 text-gray-800 hover:bg-gray-300": component.variant === "secondary",
        "bg-red-600 text-white hover:bg-red-700": component.variant === "danger",
        "bg-transparent hover:bg-gray-100": component.variant === "ghost",
        "border border-gray-300 hover:bg-gray-50": component.variant === "outline",
        "opacity-50 cursor-not-allowed": component.disabled
      });
      return /* @__PURE__ */ jsx(
        "button",
        {
          type: "button",
          className: btnClasses,
          disabled: component.disabled,
          onClick: () => handleButtonClick(component.action_id),
          children: component.label
        }
      );
    case "icon":
      const Icon = IconMap[component.name] || Info;
      return /* @__PURE__ */ jsx(Icon, { size: component.size || 24 });
    case "alert":
      const alertClasses = clsx("p-4 rounded-md border mb-4 flex items-start gap-3", {
        "bg-blue-50 border-blue-200 text-blue-800": component.variant === "info" || !component.variant,
        "bg-green-50 border-green-200 text-green-800": component.variant === "success",
        "bg-yellow-50 border-yellow-200 text-yellow-800": component.variant === "warning",
        "bg-red-50 border-red-200 text-red-800": component.variant === "error"
      });
      const AlertIcon = component.variant === "success" ? CheckCircle : component.variant === "warning" ? AlertCircle : component.variant === "error" ? XCircle : Info;
      return /* @__PURE__ */ jsxs("div", { className: alertClasses, children: [
        /* @__PURE__ */ jsx(AlertIcon, { className: "w-5 h-5 mt-0.5" }),
        /* @__PURE__ */ jsxs("div", { children: [
          /* @__PURE__ */ jsx("div", { className: "font-semibold", children: component.title }),
          component.description && /* @__PURE__ */ jsx("div", { className: "text-sm mt-1 opacity-90", children: component.description })
        ] })
      ] });
    case "card":
      const hasInputs = component.content.some(
        (c) => c.type === "text_input" || c.type === "number_input" || c.type === "select" || c.type === "switch" || c.type === "textarea"
      );
      const handleSubmit = (e) => {
        e.preventDefault();
        const formData = new FormData(e.currentTarget);
        const data = {};
        formData.forEach((value, key) => {
          data[key] = value;
        });
        const submitBtn = [...component.content, ...component.footer || []].find(
          (c) => c.type === "button"
        );
        onAction?.({
          action: "form_submit",
          action_id: submitBtn?.action_id || "form_submit",
          data
        });
      };
      const cardContent = /* @__PURE__ */ jsxs(Fragment, { children: [
        (component.title || component.description) && /* @__PURE__ */ jsxs("div", { className: "p-4 border-b dark:border-gray-700 bg-gray-50 dark:bg-gray-800", children: [
          component.title && /* @__PURE__ */ jsx("h3", { className: "font-semibold text-lg dark:text-white", children: component.title }),
          component.description && /* @__PURE__ */ jsx("p", { className: "text-gray-500 dark:text-gray-400 text-sm", children: component.description })
        ] }),
        /* @__PURE__ */ jsx("div", { className: "p-4", children: component.content.map((child, i) => /* @__PURE__ */ jsx(ComponentRenderer, { component: child }, i)) }),
        component.footer && /* @__PURE__ */ jsx("div", { className: "p-4 border-t dark:border-gray-700 bg-gray-50 dark:bg-gray-800 flex gap-2 justify-end", children: component.footer.map((child, i) => /* @__PURE__ */ jsx(ComponentRenderer, { component: child }, i)) })
      ] });
      return hasInputs ? /* @__PURE__ */ jsx("form", { onSubmit: handleSubmit, className: "bg-white dark:bg-gray-900 rounded-lg border dark:border-gray-700 shadow-sm overflow-hidden mb-4", children: cardContent }) : /* @__PURE__ */ jsx("div", { className: "bg-white dark:bg-gray-900 rounded-lg border dark:border-gray-700 shadow-sm overflow-hidden mb-4", children: cardContent });
    case "stack":
      const stackClasses = clsx("flex", {
        "flex-col": component.direction === "vertical",
        "flex-row": component.direction === "horizontal"
      });
      return /* @__PURE__ */ jsx("div", { className: stackClasses, style: { gap: (component.gap || 4) * 4 }, children: component.children.map((child, i) => /* @__PURE__ */ jsx(ComponentRenderer, { component: child }, i)) });
    case "text_input":
      const inputType = component.input_type || "text";
      return /* @__PURE__ */ jsxs("div", { className: "mb-3", children: [
        /* @__PURE__ */ jsx("label", { className: "block text-sm font-medium text-gray-700 dark:text-gray-300 mb-1", children: component.label }),
        /* @__PURE__ */ jsx(
          "input",
          {
            type: inputType,
            name: component.name,
            placeholder: component.placeholder,
            defaultValue: component.default_value,
            required: component.required,
            className: clsx("w-full px-3 py-2 border rounded-md focus:ring-2 focus:ring-blue-500 focus:border-blue-500 outline-none bg-white dark:bg-gray-800 dark:border-gray-600 dark:text-white", {
              "border-red-500 focus:ring-red-500 focus:border-red-500": component.error
            })
          }
        ),
        component.error && /* @__PURE__ */ jsx("p", { className: "text-red-500 dark:text-red-400 text-sm mt-1", children: component.error })
      ] });
    case "number_input":
      return /* @__PURE__ */ jsxs("div", { className: "mb-3", children: [
        /* @__PURE__ */ jsx("label", { className: "block text-sm font-medium text-gray-700 mb-1", children: component.label }),
        /* @__PURE__ */ jsx(
          "input",
          {
            type: "number",
            name: component.name,
            min: component.min,
            max: component.max,
            step: component.step,
            required: component.required,
            className: clsx("w-full px-3 py-2 border rounded-md focus:ring-2 focus:ring-blue-500 focus:border-blue-500 outline-none", {
              "border-red-500 focus:ring-red-500 focus:border-red-500": component.error
            })
          }
        ),
        component.error && /* @__PURE__ */ jsx("p", { className: "text-red-500 text-sm mt-1", children: component.error })
      ] });
    case "select":
      return /* @__PURE__ */ jsxs("div", { className: "mb-3", children: [
        /* @__PURE__ */ jsx("label", { className: "block text-sm font-medium text-gray-700 mb-1", children: component.label }),
        /* @__PURE__ */ jsxs(
          "select",
          {
            name: component.name,
            required: component.required,
            className: clsx("w-full px-3 py-2 border rounded-md focus:ring-2 focus:ring-blue-500 focus:border-blue-500 outline-none", {
              "border-red-500 focus:ring-red-500 focus:border-red-500": component.error
            }),
            children: [
              /* @__PURE__ */ jsx("option", { value: "", children: "Select..." }),
              component.options.map((opt, i) => /* @__PURE__ */ jsx("option", { value: opt.value, children: opt.label }, i))
            ]
          }
        ),
        component.error && /* @__PURE__ */ jsx("p", { className: "text-red-500 text-sm mt-1", children: component.error })
      ] });
    case "switch":
      return /* @__PURE__ */ jsxs("div", { className: "mb-3 flex items-center", children: [
        /* @__PURE__ */ jsx(
          "input",
          {
            type: "checkbox",
            name: component.name,
            defaultChecked: component.default_checked,
            className: "h-4 w-4 rounded border-gray-300 text-blue-600 focus:ring-blue-500"
          }
        ),
        /* @__PURE__ */ jsx("label", { className: "ml-2 text-sm font-medium text-gray-700", children: component.label })
      ] });
    case "multi_select":
      return /* @__PURE__ */ jsxs("div", { className: "mb-3", children: [
        /* @__PURE__ */ jsx("label", { className: "block text-sm font-medium text-gray-700 mb-1", children: component.label }),
        /* @__PURE__ */ jsx(
          "select",
          {
            name: component.name,
            multiple: true,
            required: component.required,
            size: Math.min(component.options.length, 5),
            className: "w-full px-3 py-2 border rounded-md focus:ring-2 focus:ring-blue-500 focus:border-blue-500 outline-none",
            children: component.options.map((opt, i) => /* @__PURE__ */ jsx("option", { value: opt.value, children: opt.label }, i))
          }
        )
      ] });
    case "date_input":
      return /* @__PURE__ */ jsxs("div", { className: "mb-3", children: [
        /* @__PURE__ */ jsx("label", { className: "block text-sm font-medium text-gray-700 mb-1", children: component.label }),
        /* @__PURE__ */ jsx(
          "input",
          {
            type: "date",
            name: component.name,
            required: component.required,
            className: "w-full px-3 py-2 border rounded-md focus:ring-2 focus:ring-blue-500 focus:border-blue-500 outline-none"
          }
        )
      ] });
    case "slider":
      return /* @__PURE__ */ jsxs("div", { className: "mb-3", children: [
        /* @__PURE__ */ jsx("label", { className: "block text-sm font-medium text-gray-700 mb-1", children: component.label }),
        /* @__PURE__ */ jsx(
          "input",
          {
            type: "range",
            name: component.name,
            min: component.min,
            max: component.max,
            step: component.step,
            defaultValue: component.default_value,
            className: "w-full h-2 bg-gray-200 rounded-lg appearance-none cursor-pointer"
          }
        )
      ] });
    case "progress":
      return /* @__PURE__ */ jsxs("div", { className: "mb-3", children: [
        component.label && /* @__PURE__ */ jsx("div", { className: "text-sm text-gray-600 mb-1", children: component.label }),
        /* @__PURE__ */ jsx("div", { className: "w-full bg-gray-200 rounded-full h-2.5", children: /* @__PURE__ */ jsx(
          "div",
          {
            className: "bg-blue-600 h-2.5 rounded-full transition-all",
            style: { width: `${component.value}%` }
          }
        ) })
      ] });
    case "textarea":
      return /* @__PURE__ */ jsxs("div", { className: "mb-3", children: [
        /* @__PURE__ */ jsx("label", { className: "block text-sm font-medium text-gray-700 dark:text-gray-300 mb-1", children: component.label }),
        /* @__PURE__ */ jsx(
          "textarea",
          {
            name: component.name,
            placeholder: component.placeholder,
            rows: component.rows || 4,
            required: component.required,
            defaultValue: component.default_value,
            className: clsx("w-full px-3 py-2 border rounded-md focus:ring-2 focus:ring-blue-500 focus:border-blue-500 outline-none bg-white dark:bg-gray-800 dark:border-gray-600 dark:text-white resize-y", {
              "border-red-500 focus:ring-red-500 focus:border-red-500": component.error
            })
          }
        ),
        component.error && /* @__PURE__ */ jsx("p", { className: "text-red-500 dark:text-red-400 text-sm mt-1", children: component.error })
      ] });
    case "spinner":
      const spinnerSizes = {
        small: "w-4 h-4",
        medium: "w-8 h-8",
        large: "w-12 h-12"
      };
      return /* @__PURE__ */ jsxs("div", { className: "flex items-center gap-2", children: [
        /* @__PURE__ */ jsx("div", { className: clsx("animate-spin rounded-full border-2 border-blue-600 border-t-transparent", spinnerSizes[component.size || "medium"]) }),
        component.label && /* @__PURE__ */ jsx("span", { className: "text-gray-600 dark:text-gray-400", children: component.label })
      ] });
    case "skeleton":
      return /* @__PURE__ */ jsx(
        "div",
        {
          className: clsx("animate-pulse bg-gray-200 dark:bg-gray-700", {
            "h-4 rounded": component.variant === "text" || !component.variant,
            "rounded-full aspect-square": component.variant === "circle",
            "rounded": component.variant === "rectangle"
          }),
          style: { width: component.width || "100%", height: component.height }
        }
      );
    case "toast":
      const toastClasses = clsx("fixed bottom-4 right-4 p-4 rounded-lg shadow-lg flex items-center gap-3 z-50", {
        "bg-blue-50 border border-blue-200 text-blue-800": component.variant === "info" || !component.variant,
        "bg-green-50 border border-green-200 text-green-800": component.variant === "success",
        "bg-yellow-50 border border-yellow-200 text-yellow-800": component.variant === "warning",
        "bg-red-50 border border-red-200 text-red-800": component.variant === "error"
      });
      const ToastIcon = component.variant === "success" ? CheckCircle : component.variant === "warning" ? AlertCircle : component.variant === "error" ? XCircle : Info;
      return /* @__PURE__ */ jsxs("div", { className: toastClasses, children: [
        /* @__PURE__ */ jsx(ToastIcon, { className: "w-5 h-5" }),
        /* @__PURE__ */ jsx("span", { children: component.message }),
        component.dismissible !== false && /* @__PURE__ */ jsx(
          "button",
          {
            onClick: () => onAction?.({ action: "button_click", action_id: "toast_dismiss" }),
            className: "ml-2 text-gray-500 hover:text-gray-700",
            children: /* @__PURE__ */ jsx(XCircle, { className: "w-4 h-4" })
          }
        )
      ] });
    case "modal":
      const modalSizes = {
        small: "max-w-sm",
        medium: "max-w-lg",
        large: "max-w-2xl",
        full: "max-w-full mx-4"
      };
      return /* @__PURE__ */ jsx("div", { className: "fixed inset-0 bg-black/50 flex items-center justify-center z-50", children: /* @__PURE__ */ jsxs("div", { className: clsx("bg-white dark:bg-gray-900 rounded-lg shadow-xl w-full", modalSizes[component.size || "medium"]), children: [
        /* @__PURE__ */ jsxs("div", { className: "p-4 border-b dark:border-gray-700 flex justify-between items-center", children: [
          /* @__PURE__ */ jsx("h3", { className: "font-semibold text-lg dark:text-white", children: component.title }),
          component.closable !== false && /* @__PURE__ */ jsx(
            "button",
            {
              onClick: () => onAction?.({ action: "button_click", action_id: "modal_close" }),
              className: "text-gray-500 hover:text-gray-700 dark:text-gray-400 dark:hover:text-gray-200",
              children: /* @__PURE__ */ jsx(XCircle, { className: "w-5 h-5" })
            }
          )
        ] }),
        /* @__PURE__ */ jsx("div", { className: "p-4", children: component.content.map((child, i) => /* @__PURE__ */ jsx(ComponentRenderer, { component: child }, i)) }),
        component.footer && /* @__PURE__ */ jsx("div", { className: "p-4 border-t dark:border-gray-700 flex justify-end gap-2", children: component.footer.map((child, i) => /* @__PURE__ */ jsx(ComponentRenderer, { component: child }, i)) })
      ] }) });
    case "grid":
      return /* @__PURE__ */ jsx(
        "div",
        {
          className: "grid gap-4 mb-4",
          style: { gridTemplateColumns: `repeat(${component.columns || 2}, 1fr)` },
          children: component.children.map((child, i) => /* @__PURE__ */ jsx(ComponentRenderer, { component: child }, i))
        }
      );
    case "list":
      return /* @__PURE__ */ jsx("ul", { className: "space-y-2 mb-4 list-disc list-inside", children: component.items.map((item, i) => /* @__PURE__ */ jsx("li", { className: "text-gray-700", children: item }, i)) });
    case "key_value":
      return /* @__PURE__ */ jsx("dl", { className: "grid grid-cols-2 gap-x-4 gap-y-2 mb-4", children: component.pairs.map((pair, i) => /* @__PURE__ */ jsxs(React.Fragment, { children: [
        /* @__PURE__ */ jsxs("dt", { className: "font-medium text-gray-700", children: [
          pair.key,
          ":"
        ] }),
        /* @__PURE__ */ jsx("dd", { className: "text-gray-900", children: pair.value })
      ] }, i)) });
    case "tabs":
      const [activeTab, setActiveTab] = React.useState(0);
      return /* @__PURE__ */ jsxs("div", { className: "mb-4", children: [
        /* @__PURE__ */ jsx("div", { className: "border-b border-gray-200", children: /* @__PURE__ */ jsx("nav", { className: "flex space-x-4", children: component.tabs.map((tab, i) => /* @__PURE__ */ jsx(
          "button",
          {
            onClick: () => setActiveTab(i),
            className: clsx("px-4 py-2 border-b-2 font-medium text-sm transition-colors", {
              "border-blue-600 text-blue-600": activeTab === i,
              "border-transparent text-gray-500 hover:text-gray-700": activeTab !== i
            }),
            children: tab.label
          },
          i
        )) }) }),
        /* @__PURE__ */ jsx("div", { className: "p-4", children: component.tabs[activeTab].content.map(
          (child, i) => /* @__PURE__ */ jsx(ComponentRenderer, { component: child }, i)
        ) })
      ] });
    case "table":
      const [sortColumn, setSortColumn] = React.useState(null);
      const [sortDirection, setSortDirection] = React.useState("asc");
      const [currentPage, setCurrentPage] = React.useState(0);
      const handleSort = (accessorKey) => {
        if (!component.sortable) return;
        if (sortColumn === accessorKey) {
          setSortDirection(sortDirection === "asc" ? "desc" : "asc");
        } else {
          setSortColumn(accessorKey);
          setSortDirection("asc");
        }
      };
      let tableData = [...component.data];
      if (sortColumn) {
        tableData.sort((a, b) => {
          const aVal = a[sortColumn] ?? "";
          const bVal = b[sortColumn] ?? "";
          const cmp = String(aVal).localeCompare(String(bVal));
          return sortDirection === "asc" ? cmp : -cmp;
        });
      }
      const pageSize = component.page_size || tableData.length;
      const totalPages = Math.ceil(tableData.length / pageSize);
      const paginatedData = tableData.slice(currentPage * pageSize, (currentPage + 1) * pageSize);
      return /* @__PURE__ */ jsxs("div", { className: "mb-4 overflow-x-auto", children: [
        /* @__PURE__ */ jsxs("table", { className: clsx("min-w-full divide-y divide-gray-200 dark:divide-gray-700 border dark:border-gray-700 rounded-lg overflow-hidden"), children: [
          /* @__PURE__ */ jsx("thead", { className: "bg-gray-50 dark:bg-gray-800", children: /* @__PURE__ */ jsx("tr", { children: component.columns.map((col, i) => /* @__PURE__ */ jsxs(
            "th",
            {
              onClick: () => handleSort(col.accessor_key),
              className: clsx(
                "px-4 py-3 text-left text-xs font-medium text-gray-500 dark:text-gray-400 uppercase tracking-wider",
                component.sortable && col.sortable !== false && "cursor-pointer hover:bg-gray-100 dark:hover:bg-gray-700"
              ),
              children: [
                col.header,
                sortColumn === col.accessor_key && /* @__PURE__ */ jsx("span", { className: "ml-1", children: sortDirection === "asc" ? "\u2191" : "\u2193" })
              ]
            },
            i
          )) }) }),
          /* @__PURE__ */ jsx("tbody", { className: "bg-white dark:bg-gray-900 divide-y divide-gray-200 dark:divide-gray-700", children: paginatedData.map((row, ri) => /* @__PURE__ */ jsx("tr", { className: clsx(
            "hover:bg-gray-50 dark:hover:bg-gray-800",
            component.striped && ri % 2 === 1 && "bg-gray-50 dark:bg-gray-800/50"
          ), children: component.columns.map((col, ci) => /* @__PURE__ */ jsx("td", { className: "px-4 py-3 text-sm text-gray-700 dark:text-gray-300", children: String(row[col.accessor_key] ?? "") }, ci)) }, ri)) })
        ] }),
        component.page_size && totalPages > 1 && /* @__PURE__ */ jsxs("div", { className: "flex items-center justify-between mt-2 px-2", children: [
          /* @__PURE__ */ jsxs("span", { className: "text-sm text-gray-500 dark:text-gray-400", children: [
            "Page ",
            currentPage + 1,
            " of ",
            totalPages
          ] }),
          /* @__PURE__ */ jsxs("div", { className: "flex gap-2", children: [
            /* @__PURE__ */ jsx(
              "button",
              {
                onClick: () => setCurrentPage(Math.max(0, currentPage - 1)),
                disabled: currentPage === 0,
                className: "px-3 py-1 text-sm border rounded hover:bg-gray-100 dark:hover:bg-gray-700 disabled:opacity-50 dark:border-gray-600 dark:text-gray-300",
                children: "Previous"
              }
            ),
            /* @__PURE__ */ jsx(
              "button",
              {
                onClick: () => setCurrentPage(Math.min(totalPages - 1, currentPage + 1)),
                disabled: currentPage === totalPages - 1,
                className: "px-3 py-1 text-sm border rounded hover:bg-gray-100 dark:hover:bg-gray-700 disabled:opacity-50 dark:border-gray-600 dark:text-gray-300",
                children: "Next"
              }
            )
          ] })
        ] })
      ] });
    case "chart":
      const DEFAULT_COLORS = ["#3B82F6", "#10B981", "#F59E0B", "#EF4444", "#8B5CF6", "#EC4899", "#06B6D4"];
      const chartColors = component.colors || DEFAULT_COLORS;
      const chartKind = component.kind || "bar";
      const showLegend = component.show_legend !== false;
      return /* @__PURE__ */ jsxs("div", { className: "mb-4 p-4 bg-white dark:bg-gray-900 border dark:border-gray-700 rounded-lg", children: [
        component.title && /* @__PURE__ */ jsx("h4", { className: "font-semibold text-lg mb-4 dark:text-white", children: component.title }),
        /* @__PURE__ */ jsx(ResponsiveContainer, { width: "100%", height: 300, children: chartKind === "line" ? /* @__PURE__ */ jsxs(LineChart, { data: component.data, children: [
          /* @__PURE__ */ jsx(CartesianGrid, { strokeDasharray: "3 3" }),
          /* @__PURE__ */ jsx(XAxis, { dataKey: component.x_key, label: component.x_label ? { value: component.x_label, position: "bottom" } : void 0 }),
          /* @__PURE__ */ jsx(YAxis, { label: component.y_label ? { value: component.y_label, angle: -90, position: "insideLeft" } : void 0 }),
          /* @__PURE__ */ jsx(Tooltip, {}),
          showLegend && /* @__PURE__ */ jsx(Legend, {}),
          component.y_keys.map((key, i) => /* @__PURE__ */ jsx(Line, { type: "monotone", dataKey: key, stroke: chartColors[i % chartColors.length], strokeWidth: 2 }, key))
        ] }) : chartKind === "area" ? /* @__PURE__ */ jsxs(AreaChart, { data: component.data, children: [
          /* @__PURE__ */ jsx(CartesianGrid, { strokeDasharray: "3 3" }),
          /* @__PURE__ */ jsx(XAxis, { dataKey: component.x_key, label: component.x_label ? { value: component.x_label, position: "bottom" } : void 0 }),
          /* @__PURE__ */ jsx(YAxis, { label: component.y_label ? { value: component.y_label, angle: -90, position: "insideLeft" } : void 0 }),
          /* @__PURE__ */ jsx(Tooltip, {}),
          showLegend && /* @__PURE__ */ jsx(Legend, {}),
          component.y_keys.map((key, i) => /* @__PURE__ */ jsx(Area, { type: "monotone", dataKey: key, fill: chartColors[i % chartColors.length], fillOpacity: 0.6, stroke: chartColors[i % chartColors.length] }, key))
        ] }) : chartKind === "pie" ? /* @__PURE__ */ jsxs(PieChart, { children: [
          /* @__PURE__ */ jsx(
            Pie,
            {
              data: component.data,
              dataKey: component.y_keys[0],
              nameKey: component.x_key,
              cx: "50%",
              cy: "50%",
              outerRadius: 100,
              label: ({ name, percent }) => `${name}: ${((percent ?? 0) * 100).toFixed(0)}%`,
              children: component.data.map((_, i) => /* @__PURE__ */ jsx(Cell, { fill: chartColors[i % chartColors.length] }, i))
            }
          ),
          /* @__PURE__ */ jsx(Tooltip, {}),
          showLegend && /* @__PURE__ */ jsx(Legend, {})
        ] }) : /* @__PURE__ */ jsxs(BarChart, { data: component.data, children: [
          /* @__PURE__ */ jsx(CartesianGrid, { strokeDasharray: "3 3" }),
          /* @__PURE__ */ jsx(XAxis, { dataKey: component.x_key, label: component.x_label ? { value: component.x_label, position: "bottom" } : void 0 }),
          /* @__PURE__ */ jsx(YAxis, { label: component.y_label ? { value: component.y_label, angle: -90, position: "insideLeft" } : void 0 }),
          /* @__PURE__ */ jsx(Tooltip, {}),
          showLegend && /* @__PURE__ */ jsx(Legend, {}),
          component.y_keys.map((key, i) => /* @__PURE__ */ jsx(Bar, { dataKey: key, fill: chartColors[i % chartColors.length] }, key))
        ] }) })
      ] });
    case "code_block":
      return /* @__PURE__ */ jsx("div", { className: "mb-4", children: /* @__PURE__ */ jsx("pre", { className: "bg-gray-900 text-gray-100 p-4 rounded-lg overflow-x-auto text-sm", children: /* @__PURE__ */ jsx("code", { children: component.code }) }) });
    case "image":
      return /* @__PURE__ */ jsx("div", { className: "mb-4", children: /* @__PURE__ */ jsx(
        "img",
        {
          src: component.src,
          alt: component.alt || "",
          className: "max-w-full h-auto rounded-lg"
        }
      ) });
    case "badge":
      const badgeClasses = clsx("inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium", {
        "bg-gray-100 text-gray-800": component.variant === "default" || !component.variant,
        "bg-blue-100 text-blue-800": component.variant === "info",
        "bg-green-100 text-green-800": component.variant === "success",
        "bg-yellow-100 text-yellow-800": component.variant === "warning",
        "bg-red-100 text-red-800": component.variant === "error",
        "bg-gray-200 text-gray-700": component.variant === "secondary",
        "bg-transparent border border-gray-300 text-gray-700": component.variant === "outline"
      });
      return /* @__PURE__ */ jsx("span", { className: badgeClasses, children: component.label });
    case "divider":
      return /* @__PURE__ */ jsx("hr", { className: "my-4 border-gray-200" });
    case "container":
      return /* @__PURE__ */ jsx("div", { className: "max-w-7xl mx-auto px-4 sm:px-6 lg:px-8", children: component.children.map((child, i) => /* @__PURE__ */ jsx(ComponentRenderer, { component: child }, i)) });
    default:
      return /* @__PURE__ */ jsxs("div", { className: "text-red-500 text-sm p-2 border border-red-200 rounded", children: [
        "Unknown component: ",
        component.type
      ] });
  }
};

// src/types.ts
function uiEventToMessage(event) {
  switch (event.action) {
    case "form_submit":
      return `[UI Event: Form submitted]
Action: ${event.action_id}
Data:
${JSON.stringify(event.data, null, 2)}`;
    case "button_click":
      return `[UI Event: Button clicked]
Action: ${event.action_id}`;
    case "input_change":
      return `[UI Event: Input changed]
Field: ${event.name}
Value: ${event.value}`;
    case "tab_change":
      return `[UI Event: Tab changed]
Index: ${event.index}`;
  }
}
export {
  Renderer,
  uiEventToMessage
};
