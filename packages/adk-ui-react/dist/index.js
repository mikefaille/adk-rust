"use strict";
var __create = Object.create;
var __defProp = Object.defineProperty;
var __getOwnPropDesc = Object.getOwnPropertyDescriptor;
var __getOwnPropNames = Object.getOwnPropertyNames;
var __getProtoOf = Object.getPrototypeOf;
var __hasOwnProp = Object.prototype.hasOwnProperty;
var __export = (target, all) => {
  for (var name in all)
    __defProp(target, name, { get: all[name], enumerable: true });
};
var __copyProps = (to, from, except, desc) => {
  if (from && typeof from === "object" || typeof from === "function") {
    for (let key of __getOwnPropNames(from))
      if (!__hasOwnProp.call(to, key) && key !== except)
        __defProp(to, key, { get: () => from[key], enumerable: !(desc = __getOwnPropDesc(from, key)) || desc.enumerable });
  }
  return to;
};
var __toESM = (mod, isNodeMode, target) => (target = mod != null ? __create(__getProtoOf(mod)) : {}, __copyProps(
  // If the importer is in node compatibility mode or this is not an ESM
  // file that has been converted to a CommonJS file using a Babel-
  // compatible transform (i.e. "__esModule" has not been set), then set
  // "default" to the CommonJS "module.exports" for node compatibility.
  isNodeMode || !mod || !mod.__esModule ? __defProp(target, "default", { value: mod, enumerable: true }) : target,
  mod
));
var __toCommonJS = (mod) => __copyProps(__defProp({}, "__esModule", { value: true }), mod);

// src/index.ts
var index_exports = {};
__export(index_exports, {
  Renderer: () => Renderer,
  uiEventToMessage: () => uiEventToMessage
});
module.exports = __toCommonJS(index_exports);

// src/Renderer.tsx
var import_react = __toESM(require("react"));
var import_lucide_react = require("lucide-react");
var import_react_markdown = __toESM(require("react-markdown"));
var import_clsx = __toESM(require("clsx"));
var import_recharts = require("recharts");
var import_jsx_runtime = require("react/jsx-runtime");
var IconMap = {
  "alert-circle": import_lucide_react.AlertCircle,
  "check-circle": import_lucide_react.CheckCircle,
  "info": import_lucide_react.Info,
  "x-circle": import_lucide_react.XCircle,
  "user": import_lucide_react.User,
  "mail": import_lucide_react.Mail,
  "calendar": import_lucide_react.Calendar
};
var FormContext = (0, import_react.createContext)({});
var Renderer = ({ component, onAction, theme }) => {
  const isDark = theme === "dark";
  return /* @__PURE__ */ (0, import_jsx_runtime.jsx)(FormContext.Provider, { value: { onAction }, children: /* @__PURE__ */ (0, import_jsx_runtime.jsx)("div", { className: isDark ? "dark" : "", children: /* @__PURE__ */ (0, import_jsx_runtime.jsx)(ComponentRenderer, { component }) }) });
};
var ComponentRenderer = ({ component }) => {
  const { onAction } = (0, import_react.useContext)(FormContext);
  const formRef = (0, import_react.useRef)(null);
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
        return /* @__PURE__ */ (0, import_jsx_runtime.jsx)("div", { className: "prose prose-sm dark:prose-invert max-w-none text-gray-700 dark:text-gray-300", children: /* @__PURE__ */ (0, import_jsx_runtime.jsx)(import_react_markdown.default, { children: component.content }) });
      }
      const Tag = component.variant === "h1" ? "h1" : component.variant === "h2" ? "h2" : component.variant === "h3" ? "h3" : component.variant === "h4" ? "h4" : component.variant === "code" ? "code" : "p";
      const classes = (0, import_clsx.default)({
        "text-4xl font-bold mb-4 dark:text-white": component.variant === "h1",
        "text-3xl font-bold mb-3 dark:text-white": component.variant === "h2",
        "text-2xl font-bold mb-2 dark:text-white": component.variant === "h3",
        "text-xl font-bold mb-2 dark:text-white": component.variant === "h4",
        "font-mono bg-gray-100 dark:bg-gray-800 p-1 rounded dark:text-gray-100": component.variant === "code",
        "text-sm text-gray-500 dark:text-gray-400": component.variant === "caption"
      });
      return /* @__PURE__ */ (0, import_jsx_runtime.jsx)(Tag, { className: classes, children: component.content });
    case "button":
      const btnClasses = (0, import_clsx.default)("px-4 py-2 rounded font-medium transition-colors", {
        "bg-blue-600 text-white hover:bg-blue-700": component.variant === "primary" || !component.variant,
        "bg-gray-200 text-gray-800 hover:bg-gray-300": component.variant === "secondary",
        "bg-red-600 text-white hover:bg-red-700": component.variant === "danger",
        "bg-transparent hover:bg-gray-100": component.variant === "ghost",
        "border border-gray-300 hover:bg-gray-50": component.variant === "outline",
        "opacity-50 cursor-not-allowed": component.disabled
      });
      return /* @__PURE__ */ (0, import_jsx_runtime.jsx)(
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
      const Icon = IconMap[component.name] || import_lucide_react.Info;
      return /* @__PURE__ */ (0, import_jsx_runtime.jsx)(Icon, { size: component.size || 24 });
    case "alert":
      const alertClasses = (0, import_clsx.default)("p-4 rounded-md border mb-4 flex items-start gap-3", {
        "bg-blue-50 border-blue-200 text-blue-800": component.variant === "info" || !component.variant,
        "bg-green-50 border-green-200 text-green-800": component.variant === "success",
        "bg-yellow-50 border-yellow-200 text-yellow-800": component.variant === "warning",
        "bg-red-50 border-red-200 text-red-800": component.variant === "error"
      });
      const AlertIcon = component.variant === "success" ? import_lucide_react.CheckCircle : component.variant === "warning" ? import_lucide_react.AlertCircle : component.variant === "error" ? import_lucide_react.XCircle : import_lucide_react.Info;
      return /* @__PURE__ */ (0, import_jsx_runtime.jsxs)("div", { className: alertClasses, children: [
        /* @__PURE__ */ (0, import_jsx_runtime.jsx)(AlertIcon, { className: "w-5 h-5 mt-0.5" }),
        /* @__PURE__ */ (0, import_jsx_runtime.jsxs)("div", { children: [
          /* @__PURE__ */ (0, import_jsx_runtime.jsx)("div", { className: "font-semibold", children: component.title }),
          component.description && /* @__PURE__ */ (0, import_jsx_runtime.jsx)("div", { className: "text-sm mt-1 opacity-90", children: component.description })
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
      const cardContent = /* @__PURE__ */ (0, import_jsx_runtime.jsxs)(import_jsx_runtime.Fragment, { children: [
        (component.title || component.description) && /* @__PURE__ */ (0, import_jsx_runtime.jsxs)("div", { className: "p-4 border-b dark:border-gray-700 bg-gray-50 dark:bg-gray-800", children: [
          component.title && /* @__PURE__ */ (0, import_jsx_runtime.jsx)("h3", { className: "font-semibold text-lg dark:text-white", children: component.title }),
          component.description && /* @__PURE__ */ (0, import_jsx_runtime.jsx)("p", { className: "text-gray-500 dark:text-gray-400 text-sm", children: component.description })
        ] }),
        /* @__PURE__ */ (0, import_jsx_runtime.jsx)("div", { className: "p-4", children: component.content.map((child, i) => /* @__PURE__ */ (0, import_jsx_runtime.jsx)(ComponentRenderer, { component: child }, i)) }),
        component.footer && /* @__PURE__ */ (0, import_jsx_runtime.jsx)("div", { className: "p-4 border-t dark:border-gray-700 bg-gray-50 dark:bg-gray-800 flex gap-2 justify-end", children: component.footer.map((child, i) => /* @__PURE__ */ (0, import_jsx_runtime.jsx)(ComponentRenderer, { component: child }, i)) })
      ] });
      return hasInputs ? /* @__PURE__ */ (0, import_jsx_runtime.jsx)("form", { onSubmit: handleSubmit, className: "bg-white dark:bg-gray-900 rounded-lg border dark:border-gray-700 shadow-sm overflow-hidden mb-4", children: cardContent }) : /* @__PURE__ */ (0, import_jsx_runtime.jsx)("div", { className: "bg-white dark:bg-gray-900 rounded-lg border dark:border-gray-700 shadow-sm overflow-hidden mb-4", children: cardContent });
    case "stack":
      const stackClasses = (0, import_clsx.default)("flex", {
        "flex-col": component.direction === "vertical",
        "flex-row": component.direction === "horizontal"
      });
      return /* @__PURE__ */ (0, import_jsx_runtime.jsx)("div", { className: stackClasses, style: { gap: (component.gap || 4) * 4 }, children: component.children.map((child, i) => /* @__PURE__ */ (0, import_jsx_runtime.jsx)(ComponentRenderer, { component: child }, i)) });
    case "text_input":
      const inputType = component.input_type || "text";
      return /* @__PURE__ */ (0, import_jsx_runtime.jsxs)("div", { className: "mb-3", children: [
        /* @__PURE__ */ (0, import_jsx_runtime.jsx)("label", { className: "block text-sm font-medium text-gray-700 dark:text-gray-300 mb-1", children: component.label }),
        /* @__PURE__ */ (0, import_jsx_runtime.jsx)(
          "input",
          {
            type: inputType,
            name: component.name,
            placeholder: component.placeholder,
            defaultValue: component.default_value,
            required: component.required,
            className: (0, import_clsx.default)("w-full px-3 py-2 border rounded-md focus:ring-2 focus:ring-blue-500 focus:border-blue-500 outline-none bg-white dark:bg-gray-800 dark:border-gray-600 dark:text-white", {
              "border-red-500 focus:ring-red-500 focus:border-red-500": component.error
            })
          }
        ),
        component.error && /* @__PURE__ */ (0, import_jsx_runtime.jsx)("p", { className: "text-red-500 dark:text-red-400 text-sm mt-1", children: component.error })
      ] });
    case "number_input":
      return /* @__PURE__ */ (0, import_jsx_runtime.jsxs)("div", { className: "mb-3", children: [
        /* @__PURE__ */ (0, import_jsx_runtime.jsx)("label", { className: "block text-sm font-medium text-gray-700 mb-1", children: component.label }),
        /* @__PURE__ */ (0, import_jsx_runtime.jsx)(
          "input",
          {
            type: "number",
            name: component.name,
            min: component.min,
            max: component.max,
            step: component.step,
            required: component.required,
            className: (0, import_clsx.default)("w-full px-3 py-2 border rounded-md focus:ring-2 focus:ring-blue-500 focus:border-blue-500 outline-none", {
              "border-red-500 focus:ring-red-500 focus:border-red-500": component.error
            })
          }
        ),
        component.error && /* @__PURE__ */ (0, import_jsx_runtime.jsx)("p", { className: "text-red-500 text-sm mt-1", children: component.error })
      ] });
    case "select":
      return /* @__PURE__ */ (0, import_jsx_runtime.jsxs)("div", { className: "mb-3", children: [
        /* @__PURE__ */ (0, import_jsx_runtime.jsx)("label", { className: "block text-sm font-medium text-gray-700 mb-1", children: component.label }),
        /* @__PURE__ */ (0, import_jsx_runtime.jsxs)(
          "select",
          {
            name: component.name,
            required: component.required,
            className: (0, import_clsx.default)("w-full px-3 py-2 border rounded-md focus:ring-2 focus:ring-blue-500 focus:border-blue-500 outline-none", {
              "border-red-500 focus:ring-red-500 focus:border-red-500": component.error
            }),
            children: [
              /* @__PURE__ */ (0, import_jsx_runtime.jsx)("option", { value: "", children: "Select..." }),
              component.options.map((opt, i) => /* @__PURE__ */ (0, import_jsx_runtime.jsx)("option", { value: opt.value, children: opt.label }, i))
            ]
          }
        ),
        component.error && /* @__PURE__ */ (0, import_jsx_runtime.jsx)("p", { className: "text-red-500 text-sm mt-1", children: component.error })
      ] });
    case "switch":
      return /* @__PURE__ */ (0, import_jsx_runtime.jsxs)("div", { className: "mb-3 flex items-center", children: [
        /* @__PURE__ */ (0, import_jsx_runtime.jsx)(
          "input",
          {
            type: "checkbox",
            name: component.name,
            defaultChecked: component.default_checked,
            className: "h-4 w-4 rounded border-gray-300 text-blue-600 focus:ring-blue-500"
          }
        ),
        /* @__PURE__ */ (0, import_jsx_runtime.jsx)("label", { className: "ml-2 text-sm font-medium text-gray-700", children: component.label })
      ] });
    case "multi_select":
      return /* @__PURE__ */ (0, import_jsx_runtime.jsxs)("div", { className: "mb-3", children: [
        /* @__PURE__ */ (0, import_jsx_runtime.jsx)("label", { className: "block text-sm font-medium text-gray-700 mb-1", children: component.label }),
        /* @__PURE__ */ (0, import_jsx_runtime.jsx)(
          "select",
          {
            name: component.name,
            multiple: true,
            required: component.required,
            size: Math.min(component.options.length, 5),
            className: "w-full px-3 py-2 border rounded-md focus:ring-2 focus:ring-blue-500 focus:border-blue-500 outline-none",
            children: component.options.map((opt, i) => /* @__PURE__ */ (0, import_jsx_runtime.jsx)("option", { value: opt.value, children: opt.label }, i))
          }
        )
      ] });
    case "date_input":
      return /* @__PURE__ */ (0, import_jsx_runtime.jsxs)("div", { className: "mb-3", children: [
        /* @__PURE__ */ (0, import_jsx_runtime.jsx)("label", { className: "block text-sm font-medium text-gray-700 mb-1", children: component.label }),
        /* @__PURE__ */ (0, import_jsx_runtime.jsx)(
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
      return /* @__PURE__ */ (0, import_jsx_runtime.jsxs)("div", { className: "mb-3", children: [
        /* @__PURE__ */ (0, import_jsx_runtime.jsx)("label", { className: "block text-sm font-medium text-gray-700 mb-1", children: component.label }),
        /* @__PURE__ */ (0, import_jsx_runtime.jsx)(
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
      return /* @__PURE__ */ (0, import_jsx_runtime.jsxs)("div", { className: "mb-3", children: [
        component.label && /* @__PURE__ */ (0, import_jsx_runtime.jsx)("div", { className: "text-sm text-gray-600 mb-1", children: component.label }),
        /* @__PURE__ */ (0, import_jsx_runtime.jsx)("div", { className: "w-full bg-gray-200 rounded-full h-2.5", children: /* @__PURE__ */ (0, import_jsx_runtime.jsx)(
          "div",
          {
            className: "bg-blue-600 h-2.5 rounded-full transition-all",
            style: { width: `${component.value}%` }
          }
        ) })
      ] });
    case "textarea":
      return /* @__PURE__ */ (0, import_jsx_runtime.jsxs)("div", { className: "mb-3", children: [
        /* @__PURE__ */ (0, import_jsx_runtime.jsx)("label", { className: "block text-sm font-medium text-gray-700 dark:text-gray-300 mb-1", children: component.label }),
        /* @__PURE__ */ (0, import_jsx_runtime.jsx)(
          "textarea",
          {
            name: component.name,
            placeholder: component.placeholder,
            rows: component.rows || 4,
            required: component.required,
            defaultValue: component.default_value,
            className: (0, import_clsx.default)("w-full px-3 py-2 border rounded-md focus:ring-2 focus:ring-blue-500 focus:border-blue-500 outline-none bg-white dark:bg-gray-800 dark:border-gray-600 dark:text-white resize-y", {
              "border-red-500 focus:ring-red-500 focus:border-red-500": component.error
            })
          }
        ),
        component.error && /* @__PURE__ */ (0, import_jsx_runtime.jsx)("p", { className: "text-red-500 dark:text-red-400 text-sm mt-1", children: component.error })
      ] });
    case "spinner":
      const spinnerSizes = {
        small: "w-4 h-4",
        medium: "w-8 h-8",
        large: "w-12 h-12"
      };
      return /* @__PURE__ */ (0, import_jsx_runtime.jsxs)("div", { className: "flex items-center gap-2", children: [
        /* @__PURE__ */ (0, import_jsx_runtime.jsx)("div", { className: (0, import_clsx.default)("animate-spin rounded-full border-2 border-blue-600 border-t-transparent", spinnerSizes[component.size || "medium"]) }),
        component.label && /* @__PURE__ */ (0, import_jsx_runtime.jsx)("span", { className: "text-gray-600 dark:text-gray-400", children: component.label })
      ] });
    case "skeleton":
      return /* @__PURE__ */ (0, import_jsx_runtime.jsx)(
        "div",
        {
          className: (0, import_clsx.default)("animate-pulse bg-gray-200 dark:bg-gray-700", {
            "h-4 rounded": component.variant === "text" || !component.variant,
            "rounded-full aspect-square": component.variant === "circle",
            "rounded": component.variant === "rectangle"
          }),
          style: { width: component.width || "100%", height: component.height }
        }
      );
    case "toast":
      const toastClasses = (0, import_clsx.default)("fixed bottom-4 right-4 p-4 rounded-lg shadow-lg flex items-center gap-3 z-50", {
        "bg-blue-50 border border-blue-200 text-blue-800": component.variant === "info" || !component.variant,
        "bg-green-50 border border-green-200 text-green-800": component.variant === "success",
        "bg-yellow-50 border border-yellow-200 text-yellow-800": component.variant === "warning",
        "bg-red-50 border border-red-200 text-red-800": component.variant === "error"
      });
      const ToastIcon = component.variant === "success" ? import_lucide_react.CheckCircle : component.variant === "warning" ? import_lucide_react.AlertCircle : component.variant === "error" ? import_lucide_react.XCircle : import_lucide_react.Info;
      return /* @__PURE__ */ (0, import_jsx_runtime.jsxs)("div", { className: toastClasses, children: [
        /* @__PURE__ */ (0, import_jsx_runtime.jsx)(ToastIcon, { className: "w-5 h-5" }),
        /* @__PURE__ */ (0, import_jsx_runtime.jsx)("span", { children: component.message }),
        component.dismissible !== false && /* @__PURE__ */ (0, import_jsx_runtime.jsx)(
          "button",
          {
            onClick: () => onAction?.({ action: "button_click", action_id: "toast_dismiss" }),
            className: "ml-2 text-gray-500 hover:text-gray-700",
            children: /* @__PURE__ */ (0, import_jsx_runtime.jsx)(import_lucide_react.XCircle, { className: "w-4 h-4" })
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
      return /* @__PURE__ */ (0, import_jsx_runtime.jsx)("div", { className: "fixed inset-0 bg-black/50 flex items-center justify-center z-50", children: /* @__PURE__ */ (0, import_jsx_runtime.jsxs)("div", { className: (0, import_clsx.default)("bg-white dark:bg-gray-900 rounded-lg shadow-xl w-full", modalSizes[component.size || "medium"]), children: [
        /* @__PURE__ */ (0, import_jsx_runtime.jsxs)("div", { className: "p-4 border-b dark:border-gray-700 flex justify-between items-center", children: [
          /* @__PURE__ */ (0, import_jsx_runtime.jsx)("h3", { className: "font-semibold text-lg dark:text-white", children: component.title }),
          component.closable !== false && /* @__PURE__ */ (0, import_jsx_runtime.jsx)(
            "button",
            {
              onClick: () => onAction?.({ action: "button_click", action_id: "modal_close" }),
              className: "text-gray-500 hover:text-gray-700 dark:text-gray-400 dark:hover:text-gray-200",
              children: /* @__PURE__ */ (0, import_jsx_runtime.jsx)(import_lucide_react.XCircle, { className: "w-5 h-5" })
            }
          )
        ] }),
        /* @__PURE__ */ (0, import_jsx_runtime.jsx)("div", { className: "p-4", children: component.content.map((child, i) => /* @__PURE__ */ (0, import_jsx_runtime.jsx)(ComponentRenderer, { component: child }, i)) }),
        component.footer && /* @__PURE__ */ (0, import_jsx_runtime.jsx)("div", { className: "p-4 border-t dark:border-gray-700 flex justify-end gap-2", children: component.footer.map((child, i) => /* @__PURE__ */ (0, import_jsx_runtime.jsx)(ComponentRenderer, { component: child }, i)) })
      ] }) });
    case "grid":
      return /* @__PURE__ */ (0, import_jsx_runtime.jsx)(
        "div",
        {
          className: "grid gap-4 mb-4",
          style: { gridTemplateColumns: `repeat(${component.columns || 2}, 1fr)` },
          children: component.children.map((child, i) => /* @__PURE__ */ (0, import_jsx_runtime.jsx)(ComponentRenderer, { component: child }, i))
        }
      );
    case "list":
      return /* @__PURE__ */ (0, import_jsx_runtime.jsx)("ul", { className: "space-y-2 mb-4 list-disc list-inside", children: component.items.map((item, i) => /* @__PURE__ */ (0, import_jsx_runtime.jsx)("li", { className: "text-gray-700", children: item }, i)) });
    case "key_value":
      return /* @__PURE__ */ (0, import_jsx_runtime.jsx)("dl", { className: "grid grid-cols-2 gap-x-4 gap-y-2 mb-4", children: component.pairs.map((pair, i) => /* @__PURE__ */ (0, import_jsx_runtime.jsxs)(import_react.default.Fragment, { children: [
        /* @__PURE__ */ (0, import_jsx_runtime.jsxs)("dt", { className: "font-medium text-gray-700", children: [
          pair.key,
          ":"
        ] }),
        /* @__PURE__ */ (0, import_jsx_runtime.jsx)("dd", { className: "text-gray-900", children: pair.value })
      ] }, i)) });
    case "tabs":
      const [activeTab, setActiveTab] = import_react.default.useState(0);
      return /* @__PURE__ */ (0, import_jsx_runtime.jsxs)("div", { className: "mb-4", children: [
        /* @__PURE__ */ (0, import_jsx_runtime.jsx)("div", { className: "border-b border-gray-200", children: /* @__PURE__ */ (0, import_jsx_runtime.jsx)("nav", { className: "flex space-x-4", children: component.tabs.map((tab, i) => /* @__PURE__ */ (0, import_jsx_runtime.jsx)(
          "button",
          {
            onClick: () => setActiveTab(i),
            className: (0, import_clsx.default)("px-4 py-2 border-b-2 font-medium text-sm transition-colors", {
              "border-blue-600 text-blue-600": activeTab === i,
              "border-transparent text-gray-500 hover:text-gray-700": activeTab !== i
            }),
            children: tab.label
          },
          i
        )) }) }),
        /* @__PURE__ */ (0, import_jsx_runtime.jsx)("div", { className: "p-4", children: component.tabs[activeTab].content.map(
          (child, i) => /* @__PURE__ */ (0, import_jsx_runtime.jsx)(ComponentRenderer, { component: child }, i)
        ) })
      ] });
    case "table":
      const [sortColumn, setSortColumn] = import_react.default.useState(null);
      const [sortDirection, setSortDirection] = import_react.default.useState("asc");
      const [currentPage, setCurrentPage] = import_react.default.useState(0);
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
      return /* @__PURE__ */ (0, import_jsx_runtime.jsxs)("div", { className: "mb-4 overflow-x-auto", children: [
        /* @__PURE__ */ (0, import_jsx_runtime.jsxs)("table", { className: (0, import_clsx.default)("min-w-full divide-y divide-gray-200 dark:divide-gray-700 border dark:border-gray-700 rounded-lg overflow-hidden"), children: [
          /* @__PURE__ */ (0, import_jsx_runtime.jsx)("thead", { className: "bg-gray-50 dark:bg-gray-800", children: /* @__PURE__ */ (0, import_jsx_runtime.jsx)("tr", { children: component.columns.map((col, i) => /* @__PURE__ */ (0, import_jsx_runtime.jsxs)(
            "th",
            {
              onClick: () => handleSort(col.accessor_key),
              className: (0, import_clsx.default)(
                "px-4 py-3 text-left text-xs font-medium text-gray-500 dark:text-gray-400 uppercase tracking-wider",
                component.sortable && col.sortable !== false && "cursor-pointer hover:bg-gray-100 dark:hover:bg-gray-700"
              ),
              children: [
                col.header,
                sortColumn === col.accessor_key && /* @__PURE__ */ (0, import_jsx_runtime.jsx)("span", { className: "ml-1", children: sortDirection === "asc" ? "\u2191" : "\u2193" })
              ]
            },
            i
          )) }) }),
          /* @__PURE__ */ (0, import_jsx_runtime.jsx)("tbody", { className: "bg-white dark:bg-gray-900 divide-y divide-gray-200 dark:divide-gray-700", children: paginatedData.map((row, ri) => /* @__PURE__ */ (0, import_jsx_runtime.jsx)("tr", { className: (0, import_clsx.default)(
            "hover:bg-gray-50 dark:hover:bg-gray-800",
            component.striped && ri % 2 === 1 && "bg-gray-50 dark:bg-gray-800/50"
          ), children: component.columns.map((col, ci) => /* @__PURE__ */ (0, import_jsx_runtime.jsx)("td", { className: "px-4 py-3 text-sm text-gray-700 dark:text-gray-300", children: String(row[col.accessor_key] ?? "") }, ci)) }, ri)) })
        ] }),
        component.page_size && totalPages > 1 && /* @__PURE__ */ (0, import_jsx_runtime.jsxs)("div", { className: "flex items-center justify-between mt-2 px-2", children: [
          /* @__PURE__ */ (0, import_jsx_runtime.jsxs)("span", { className: "text-sm text-gray-500 dark:text-gray-400", children: [
            "Page ",
            currentPage + 1,
            " of ",
            totalPages
          ] }),
          /* @__PURE__ */ (0, import_jsx_runtime.jsxs)("div", { className: "flex gap-2", children: [
            /* @__PURE__ */ (0, import_jsx_runtime.jsx)(
              "button",
              {
                onClick: () => setCurrentPage(Math.max(0, currentPage - 1)),
                disabled: currentPage === 0,
                className: "px-3 py-1 text-sm border rounded hover:bg-gray-100 dark:hover:bg-gray-700 disabled:opacity-50 dark:border-gray-600 dark:text-gray-300",
                children: "Previous"
              }
            ),
            /* @__PURE__ */ (0, import_jsx_runtime.jsx)(
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
      return /* @__PURE__ */ (0, import_jsx_runtime.jsxs)("div", { className: "mb-4 p-4 bg-white dark:bg-gray-900 border dark:border-gray-700 rounded-lg", children: [
        component.title && /* @__PURE__ */ (0, import_jsx_runtime.jsx)("h4", { className: "font-semibold text-lg mb-4 dark:text-white", children: component.title }),
        /* @__PURE__ */ (0, import_jsx_runtime.jsx)(import_recharts.ResponsiveContainer, { width: "100%", height: 300, children: chartKind === "line" ? /* @__PURE__ */ (0, import_jsx_runtime.jsxs)(import_recharts.LineChart, { data: component.data, children: [
          /* @__PURE__ */ (0, import_jsx_runtime.jsx)(import_recharts.CartesianGrid, { strokeDasharray: "3 3" }),
          /* @__PURE__ */ (0, import_jsx_runtime.jsx)(import_recharts.XAxis, { dataKey: component.x_key, label: component.x_label ? { value: component.x_label, position: "bottom" } : void 0 }),
          /* @__PURE__ */ (0, import_jsx_runtime.jsx)(import_recharts.YAxis, { label: component.y_label ? { value: component.y_label, angle: -90, position: "insideLeft" } : void 0 }),
          /* @__PURE__ */ (0, import_jsx_runtime.jsx)(import_recharts.Tooltip, {}),
          showLegend && /* @__PURE__ */ (0, import_jsx_runtime.jsx)(import_recharts.Legend, {}),
          component.y_keys.map((key, i) => /* @__PURE__ */ (0, import_jsx_runtime.jsx)(import_recharts.Line, { type: "monotone", dataKey: key, stroke: chartColors[i % chartColors.length], strokeWidth: 2 }, key))
        ] }) : chartKind === "area" ? /* @__PURE__ */ (0, import_jsx_runtime.jsxs)(import_recharts.AreaChart, { data: component.data, children: [
          /* @__PURE__ */ (0, import_jsx_runtime.jsx)(import_recharts.CartesianGrid, { strokeDasharray: "3 3" }),
          /* @__PURE__ */ (0, import_jsx_runtime.jsx)(import_recharts.XAxis, { dataKey: component.x_key, label: component.x_label ? { value: component.x_label, position: "bottom" } : void 0 }),
          /* @__PURE__ */ (0, import_jsx_runtime.jsx)(import_recharts.YAxis, { label: component.y_label ? { value: component.y_label, angle: -90, position: "insideLeft" } : void 0 }),
          /* @__PURE__ */ (0, import_jsx_runtime.jsx)(import_recharts.Tooltip, {}),
          showLegend && /* @__PURE__ */ (0, import_jsx_runtime.jsx)(import_recharts.Legend, {}),
          component.y_keys.map((key, i) => /* @__PURE__ */ (0, import_jsx_runtime.jsx)(import_recharts.Area, { type: "monotone", dataKey: key, fill: chartColors[i % chartColors.length], fillOpacity: 0.6, stroke: chartColors[i % chartColors.length] }, key))
        ] }) : chartKind === "pie" ? /* @__PURE__ */ (0, import_jsx_runtime.jsxs)(import_recharts.PieChart, { children: [
          /* @__PURE__ */ (0, import_jsx_runtime.jsx)(
            import_recharts.Pie,
            {
              data: component.data,
              dataKey: component.y_keys[0],
              nameKey: component.x_key,
              cx: "50%",
              cy: "50%",
              outerRadius: 100,
              label: ({ name, percent }) => `${name}: ${((percent ?? 0) * 100).toFixed(0)}%`,
              children: component.data.map((_, i) => /* @__PURE__ */ (0, import_jsx_runtime.jsx)(import_recharts.Cell, { fill: chartColors[i % chartColors.length] }, i))
            }
          ),
          /* @__PURE__ */ (0, import_jsx_runtime.jsx)(import_recharts.Tooltip, {}),
          showLegend && /* @__PURE__ */ (0, import_jsx_runtime.jsx)(import_recharts.Legend, {})
        ] }) : /* @__PURE__ */ (0, import_jsx_runtime.jsxs)(import_recharts.BarChart, { data: component.data, children: [
          /* @__PURE__ */ (0, import_jsx_runtime.jsx)(import_recharts.CartesianGrid, { strokeDasharray: "3 3" }),
          /* @__PURE__ */ (0, import_jsx_runtime.jsx)(import_recharts.XAxis, { dataKey: component.x_key, label: component.x_label ? { value: component.x_label, position: "bottom" } : void 0 }),
          /* @__PURE__ */ (0, import_jsx_runtime.jsx)(import_recharts.YAxis, { label: component.y_label ? { value: component.y_label, angle: -90, position: "insideLeft" } : void 0 }),
          /* @__PURE__ */ (0, import_jsx_runtime.jsx)(import_recharts.Tooltip, {}),
          showLegend && /* @__PURE__ */ (0, import_jsx_runtime.jsx)(import_recharts.Legend, {}),
          component.y_keys.map((key, i) => /* @__PURE__ */ (0, import_jsx_runtime.jsx)(import_recharts.Bar, { dataKey: key, fill: chartColors[i % chartColors.length] }, key))
        ] }) })
      ] });
    case "code_block":
      return /* @__PURE__ */ (0, import_jsx_runtime.jsx)("div", { className: "mb-4", children: /* @__PURE__ */ (0, import_jsx_runtime.jsx)("pre", { className: "bg-gray-900 text-gray-100 p-4 rounded-lg overflow-x-auto text-sm", children: /* @__PURE__ */ (0, import_jsx_runtime.jsx)("code", { children: component.code }) }) });
    case "image":
      return /* @__PURE__ */ (0, import_jsx_runtime.jsx)("div", { className: "mb-4", children: /* @__PURE__ */ (0, import_jsx_runtime.jsx)(
        "img",
        {
          src: component.src,
          alt: component.alt || "",
          className: "max-w-full h-auto rounded-lg"
        }
      ) });
    case "badge":
      const badgeClasses = (0, import_clsx.default)("inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium", {
        "bg-gray-100 text-gray-800": component.variant === "default" || !component.variant,
        "bg-blue-100 text-blue-800": component.variant === "info",
        "bg-green-100 text-green-800": component.variant === "success",
        "bg-yellow-100 text-yellow-800": component.variant === "warning",
        "bg-red-100 text-red-800": component.variant === "error",
        "bg-gray-200 text-gray-700": component.variant === "secondary",
        "bg-transparent border border-gray-300 text-gray-700": component.variant === "outline"
      });
      return /* @__PURE__ */ (0, import_jsx_runtime.jsx)("span", { className: badgeClasses, children: component.label });
    case "divider":
      return /* @__PURE__ */ (0, import_jsx_runtime.jsx)("hr", { className: "my-4 border-gray-200" });
    case "container":
      return /* @__PURE__ */ (0, import_jsx_runtime.jsx)("div", { className: "max-w-7xl mx-auto px-4 sm:px-6 lg:px-8", children: component.children.map((child, i) => /* @__PURE__ */ (0, import_jsx_runtime.jsx)(ComponentRenderer, { component: child }, i)) });
    default:
      return /* @__PURE__ */ (0, import_jsx_runtime.jsxs)("div", { className: "text-red-500 text-sm p-2 border border-red-200 rounded", children: [
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
// Annotate the CommonJS export names for ESM import in node:
0 && (module.exports = {
  Renderer,
  uiEventToMessage
});
