use ldk_node::Node;
use maud::{html, Markup, DOCTYPE};

/// Helper function to check if the node is running
pub fn is_node_running(node: &Node) -> bool {
    node.status().is_running
}

pub fn layout_with_status(title: &str, content: Markup, is_running: bool) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                link rel="icon" type="image/svg+xml" href="/static/favicon.svg";
                link rel="stylesheet" type="text/css" href="/static/css/globe.css";
                title { (title) " - CDK LDK Node" }
                style {
                    "
                    :root {
                        /* Light mode (default) */
                        --background: 0 0% 100%;
                        --foreground: 222.2 84% 4.9%;
                        --card: 0 0% 100%;
                        --card-foreground: 222.2 84% 4.9%;
                        --popover: 0 0% 100%;
                        --popover-foreground: 222.2 84% 4.9%;
                        --primary: 222.2 47.4% 11.2%;
                        --primary-foreground: 210 40% 98%;
                        --secondary: 210 40% 96%;
                        --secondary-foreground: 222.2 84% 4.9%;
                        --muted: 210 40% 96%;
                        --muted-foreground: 215.4 16.3% 46.9%;
                        --accent: 210 40% 96%;
                        --accent-foreground: 222.2 84% 4.9%;
                        --destructive: 0 84.2% 60.2%;
                        --destructive-foreground: 210 40% 98%;
                        --border: 214.3 31.8% 91.4%;
                        --input: 214.3 31.8% 91.4%;
                        --ring: 222.2 84% 4.9%;
                        --radius: 0;

                        /* Typography scale */
                        --fs-title: 1.25rem;
                        --fs-label: 0.8125rem;
                        --fs-value: 1.625rem;

                        /* Line heights */
                        --lh-tight: 1.15;
                        --lh-normal: 1.4;

                        /* Font weights */
                        --fw-medium: 500;
                        --fw-semibold: 600;
                        --fw-bold: 700;

                        /* Colors */
                        --fg-primary: #0f172a;
                        --fg-muted: #6b7280;

                        /* Header text colors for light mode */
                        --header-title: #000000;
                        --header-subtitle: #333333;
                    }

                                        /* Dark mode using system preference */
                    @media (prefers-color-scheme: dark) {
                        body {
                            background: linear-gradient(rgb(23, 25, 29), rgb(18, 19, 21));
                        }

                        :root {
                            --background: 0 0% 0%;
                            --foreground: 0 0% 100%;
                            --card: 0 0% 0%;
                            --card-foreground: 0 0% 100%;
                            --popover: 0 0% 0%;
                            --popover-foreground: 0 0% 100%;
                            --primary: 0 0% 100%;
                            --primary-foreground: 0 0% 0%;
                            --secondary: 0 0% 20%;
                            --secondary-foreground: 0 0% 100%;
                            --muted: 0 0% 20%;
                            --muted-foreground: 0 0% 70%;
                            --accent: 0 0% 20%;
                            --accent-foreground: 0 0% 100%;
                            --destructive: 0 62.8% 30.6%;
                            --destructive-foreground: 0 0% 100%;
                            --border: 0 0% 20%;
                            --input: 0 0% 20%;
                            --ring: 0 0% 83.9%;

                            /* Dark mode text hierarchy colors */
                            --text-primary: #ffffff;
                            --text-secondary: #e6e6e6;
                            --text-tertiary: #cccccc;
                            --text-quaternary: #b3b3b3;
                            --text-muted: #999999;
                            --text-muted-2: #888888;
                            --text-muted-3: #666666;
                            --text-muted-4: #333333;
                            --text-subtle: #1a1a1a;

                            /* Header text colors for dark mode */
                            --header-title: #ffffff;
                            --header-subtitle: #e6e6e6;
                        }

                        /* Dark mode box styling - no borders, subtle background */
                        .card {
                            background-color: rgba(255, 255, 255, 0.03) !important;
                            border: none !important;
                        }

                        .channel-box {
                            background-color: rgba(255, 255, 255, 0.03) !important;
                            border: none !important;
                        }

                        .metric-card {
                            background-color: rgba(255, 255, 255, 0.03) !important;
                            border: none !important;
                        }

                                                .balance-item {
                            background-color: rgba(255, 255, 255, 0.03) !important;
                            border: none !important;
                        }

                        .node-info-main-container {
                            background-color: rgba(255, 255, 255, 0.03) !important;
                            border: none !important;
                        }

                        .node-avatar {
                            background-color: rgba(255, 255, 255, 0.03) !important;
                            border: none !important;
                        }

                        /* Text hierarchy colors */
                        .section-header {
                            color: var(--text-primary) !important;
                        }

                        .channel-alias {
                            color: var(--text-primary) !important;
                        }

                        .detail-label {
                            color: hsl(var(--foreground)) !important;
                            opacity: 0.5 !important;
                        }

                        .detail-value, .detail-value-amount {
                            color: hsl(var(--foreground)) !important;
                        }

                        .info-value {
                            color: var(--text-primary) !important;
                        }

                        .metric-label, .balance-label {
                            color: var(--text-muted) !important;
                        }

                        .metric-value, .balance-amount {
                            color: var(--text-primary) !important;
                        }

                        /* Page headers and section titles */
                        h1, h2, h3, h4, h5, h6 {
                            color: var(--text-primary) !important;
                        }

                        /* Form card titles */
                        .form-card h2, .form-card h3 {
                            color: var(--text-primary) !important;
                        }

                        /* Quick action cards styling */
                        .quick-action-card {
                            background-color: rgba(255, 255, 255, 0.03) !important;
                            border: none !important;
                            border-radius: 0 !important;
                            padding: 1.5rem !important;
                        }

                        /* Dark mode outline button styling */
                        .button-outline {
                            background-color: transparent !important;
                            color: var(--text-primary) !important;
                            border: 1px solid var(--text-muted) !important;
                        }

                        .button-outline:hover {
                            background-color: rgba(255, 255, 255, 0.2) !important;
                        }

                        /* Navigation dark mode styling */
                        nav {
                            background-color: transparent !important;
                            border-top: none !important;
                            border-bottom: none !important;
                        }

                                                                    }

                    /* New Header Layout Styles */
                    .header-content {
                        display: flex;
                        justify-content: space-between;
                        align-items: center;
                        padding: 0.5rem 0;
                    }

                    .header-left {
                        display: flex;
                        align-items: center;
                        gap: 1rem;
                    }

                    .header-avatar {
                        flex-shrink: 0;
                        background-color: hsl(var(--muted) / 0.3);
                        border: 1px solid hsl(var(--border));
                        border-radius: 0;
                        padding: 0.75rem;
                        display: flex;
                        align-items: center;
                        justify-content: center;
                        width: 80px;
                        height: 80px;
                    }

                    .header-avatar-image {
                        width: 48px;
                        height: 48px;
                        border-radius: 0;
                        object-fit: cover;
                        display: block;
                    }

                    .node-info {
                        display: flex;
                        flex-direction: column;
                        gap: 0.25rem;
                        padding-top: 0;
                        margin-top: 0;
                    }

                    .node-status {
                        display: flex;
                        align-items: center;
                        gap: 0.5rem;
                    }

                    .status-indicator {
                        width: 0.75rem;
                        height: 0.75rem;
                        border-radius: 50%;
                        background-color: #10b981;
                        box-shadow: 0 0 0 2px rgba(16, 185, 129, 0.2);
                    }

                    .status-indicator.status-inactive {
                        background-color: #ef4444;
                        box-shadow: 0 0 0 2px rgba(239, 68, 68, 0.2);
                    }

                    .status-text {
                        font-size: 0.875rem;
                        font-weight: 500;
                        color: #10b981;
                    }

                    .status-text.status-inactive {
                        color: #ef4444;
                    }

                    .node-title {
                        font-size: 1.875rem;
                        font-weight: 600;
                        color: var(--header-title);
                        margin: 0;
                        line-height: 1.1;
                    }

                    .node-subtitle {
                        font-size: 0.75rem;
                        color: var(--text-muted);
                        font-weight: 500;
                        letter-spacing: 0.05em;
                        text-transform: uppercase;
                    }

                    .header-right {
                        display: flex;
                        align-items: center;
                    }



                    /* Responsive header */
                    @media (max-width: 768px) {
                        header {
                            height: 180px; /* Slightly taller for better mobile layout */
                            padding: 1rem 0;
                        }

                        header .container {
                            padding: 0 1rem;
                            height: 100%;
                            display: flex;
                            align-items: center;
                            justify-content: center;
                        }

                        .header-content {
                            flex-direction: column;
                            gap: 1rem;
                            text-align: center;
                            width: 100%;
                            justify-content: center;
                        }

                        .header-left {
                            flex-direction: column;
                            text-align: center;
                            align-items: center;
                            gap: 0.75rem;
                        }

                        .header-avatar {
                            width: 64px;
                            height: 64px;
                            padding: 0.5rem;
                        }

                        .header-avatar-image {
                            width: 40px;
                            height: 40px;
                        }

                        .node-title {
                            font-size: 1.5rem;
                        }

                        .node-subtitle {
                            font-size: 0.6875rem;
                            text-align: center;
                        }

                        .node-status {
                            justify-content: center;
                        }
                    }

                    @media (max-width: 480px) {
                        header {
                            height: 160px;
                        }

                        .header-avatar {
                            width: 56px;
                            height: 56px;
                            padding: 0.375rem;
                        }

                        .header-avatar-image {
                            width: 36px;
                            height: 36px;
                        }

                        .node-title {
                            font-size: 1.25rem;
                        }

                        .node-subtitle {
                            font-size: 0.75rem;
                        }
                    }

                    /* Dark mode navigation styles */
                    @media (prefers-color-scheme: dark) {
                        nav a {
                            color: var(--text-muted) !important;
                        }

                        nav a:hover {
                            color: var(--text-secondary) !important;
                            background-color: rgba(255, 255, 255, 0.08) !important;
                            transform: translateY(-1px) !important;
                        }

                        nav a.active {
                            color: var(--text-primary) !important;
                            background-color: rgba(255, 255, 255, 0.1) !important;
                        }

                        nav a.active:hover {
                            background-color: rgba(255, 255, 255, 0.12) !important;
                            transform: translateY(-1px) !important;
                        }
                    }

                    * {
                        box-sizing: border-box;
                        margin: 0;
                        padding: 0;
                    }

                    html {
                        font-feature-settings: 'cv02', 'cv03', 'cv04', 'cv11';
                        font-variation-settings: normal;
                    }

                    body {
                        font-family: 'Inter', -apple-system, BlinkMacSystemFont, 'Segoe UI', 'Roboto', 'Oxygen', 'Ubuntu', 'Cantarell', 'Fira Sans', 'Droid Sans', 'Helvetica Neue', sans-serif;
                        font-size: 14px;
                        line-height: 1.5;
                        color: hsl(var(--foreground));
                        background-color: hsl(var(--background));
                        font-feature-settings: 'rlig' 1, 'calt' 1;
                        -webkit-font-smoothing: antialiased;
                        -moz-osx-font-smoothing: grayscale;
                        text-rendering: geometricPrecision;
                        min-height: 100vh;
                    }

                    .container {
                        max-width: 1200px;
                        margin: 0 auto;
                        padding: 0 1rem;
                    }

                    @media (min-width: 640px) {
                        .container {
                            padding: 0 2rem;
                        }
                    }

                    /* Hero section styling */
                    header {
                        position: relative;
                        background-color: hsl(var(--background));
                        background-image:
                            linear-gradient(hsl(var(--border)) 1px, transparent 1px),
                            linear-gradient(90deg, hsl(var(--border)) 1px, transparent 1px);
                        background-size: 40px 40px;
                        background-position: -1px -1px;
                        border-bottom: 1px solid hsl(var(--border));
                        margin-bottom: 2rem;
                        text-align: left;
                        width: 100%;
                        height: 200px; /* Reduced height for more compact header */
                        display: flex;
                        align-items: center;
                        justify-content: flex-start;
                    }

                    /* Subtle diamond gradient fade on edges */
                    header::before {
                        content: '';
                        position: absolute;
                        top: 0;
                        left: 0;
                        right: 0;
                        bottom: 0;
                        background:
                            linear-gradient(90deg, hsl(var(--background)) 0%, transparent 15%, transparent 85%, hsl(var(--background)) 100%),
                            linear-gradient(180deg, hsl(var(--background)) 0%, transparent 15%, transparent 85%, hsl(var(--background)) 100%);
                        pointer-events: none;
                        z-index: 1;
                    }

                    /* Dark mode header background - subtle grid with darker theme */
                    @media (prefers-color-scheme: dark) {
                        header {
                            background-color: rgb(18, 19, 21);
                            background-image:
                                linear-gradient(rgba(255, 255, 255, 0.03) 1px, transparent 1px),
                                linear-gradient(90deg, rgba(255, 255, 255, 0.03) 1px, transparent 1px);
                        }

                        header::before {
                            background:
                                linear-gradient(90deg, rgb(18, 19, 21) 0%, transparent 15%, transparent 85%, rgb(18, 19, 21) 100%),
                                linear-gradient(180deg, rgb(18, 19, 21) 0%, transparent 15%, transparent 85%, rgb(18, 19, 21) 100%);
                        }
                    }

                    /* Ensure text is positioned properly */
                    header .container {
                        position: relative;
                        top: auto;
                        left: auto;
                        transform: none;
                        z-index: 2;
                        width: 100%;
                        max-width: 1200px;
                        padding: 0 2rem;
                        display: flex;
                        align-items: center;
                        justify-content: flex-start;
                    }

                    h1 {
                        font-size: 3rem;
                        font-weight: 700;
                        line-height: 1.1;
                        letter-spacing: -0.02em;
                        color: var(--header-title);
                        margin-bottom: 1rem;
                    }

                    .subtitle {
                        font-size: 1.25rem;
                        color: var(--header-subtitle);
                        font-weight: 400;
                        max-width: 600px;
                        margin: 0 auto;
                        line-height: 1.6;
                    }


                    /* Card fade-in animation */
                    @keyframes fade-in {
                        from { opacity: 0; transform: translateY(10px); }
                        to { opacity: 1; transform: translateY(0); }
                    }

                    .card {
                        animation: fade-in 0.3s ease-out;
                    }

                    /* Corner embellishments for angular design */
                    .card::before,
                    .card::after {
                        content: '';
                        position: absolute;
                        width: 16px;
                        height: 16px;
                        border: 1px solid hsl(var(--border));
                    }

                    .card::before {
                        top: -1px;
                        left: -1px;
                        border-right: none;
                        border-bottom: none;
                    }

                    .card::after {
                        bottom: -1px;
                        right: -1px;
                        border-left: none;
                        border-top: none;
                    }

                    @media (prefers-color-scheme: dark) {
                        .card::before,
                        .card::after {
                            border-color: rgba(255, 255, 255, 0.2);
                        }
                    }

                    /* Modern Navigation Bar Styling */
                    nav {
                        background-color: hsl(var(--card));
                        border-top: 1px solid hsl(var(--border));
                        border-bottom: 1px solid hsl(var(--border));
                        border-left: none;
                        border-right: none;
                        border-radius: 0;
                        padding: 0.75rem;
                        margin-bottom: 2rem;
                    }

                    nav .container {
                        padding: 0;
                        display: flex;
                        justify-content: center;
                    }

                    nav ul {
                        list-style: none;
                        display: flex;
                        gap: 0.5rem;
                        overflow-x: auto;
                        -webkit-overflow-scrolling: touch;
                        margin: 0;
                        padding: 0;
                        justify-content: center;
                    }

                    nav li {
                        flex-shrink: 0;
                    }

                    nav a {
                        display: inline-flex;
                        align-items: center;
                        justify-content: center;
                        white-space: nowrap;
                        text-decoration: none;
                        font-size: 1rem;
                        font-weight: 600;
                        color: hsl(var(--muted-foreground));
                        padding: 1rem 1.5rem;
                        border-radius: 0;
                        transition: all 200ms cubic-bezier(0.4, 0, 0.2, 1);
                        position: relative;
                        min-height: 3rem;
                    }

                    nav a:hover {
                        color: hsl(var(--foreground));
                        background-color: hsl(var(--muted));
                    }

                    /* Light mode navigation hover states */
                    @media (prefers-color-scheme: light) {
                        nav a:hover {
                            color: hsl(var(--foreground));
                            background-color: hsl(var(--muted) / 0.8);
                            transform: translateY(-1px);
                        }
                    }

                    nav a.active {
                        color: hsl(var(--primary-foreground));
                        background-color: hsl(var(--primary));
                        font-weight: 700;
                    }

                    nav a.active:hover {
                        background-color: hsl(var(--primary) / 0.9);
                    }

                    .card {
                        position: relative;
                        background-color: hsl(var(--card));
                        border: 1px solid hsl(var(--border));
                        border-radius: 0;
                        padding: 1.5rem;
                        margin-bottom: 1.5rem;
                        box-shadow: none;
                    }

                    /* Metric cards styling - matching balance-item style */
                    .metrics-container {
                        display: flex;
                        gap: 1rem;
                        margin: 1rem 0;
                        flex-wrap: wrap;
                    }

                    .metric-card {
                        position: relative;
                        flex: 1;
                        min-width: 200px;
                        text-align: center;
                        padding: 1rem;
                        background-color: hsl(var(--muted) / 0.3);
                        border-radius: 0;
                        border: 1px solid hsl(var(--border));
                    }

                    .metric-card::before,
                    .metric-card::after {
                        content: '';
                        position: absolute;
                        width: 12px;
                        height: 12px;
                        border: 1px solid hsl(var(--border));
                    }

                    .metric-card::before {
                        top: -1px;
                        left: -1px;
                        border-right: none;
                        border-bottom: none;
                    }

                    .metric-card::after {
                        bottom: -1px;
                        right: -1px;
                        border-left: none;
                        border-top: none;
                    }

                    @media (prefers-color-scheme: dark) {
                        .metric-card::before,
                        .metric-card::after {
                            border-color: rgba(255, 255, 255, 0.2);
                        }
                    }

                    .metric-value {
                        font-size: 1.5rem;
                        font-weight: 600;
                        color: hsl(var(--foreground));
                        margin-bottom: 0.5rem;
                        line-height: 1.2;
                    }

                    .metric-label {
                        font-size: 0.875rem;
                        color: hsl(var(--muted-foreground));
                        font-weight: 400;
                    }

                    .card h2,
                    .section-title,
                    h2 {
                        font-size: var(--fs-title);
                        line-height: var(--lh-tight);
                        font-weight: var(--fw-semibold);
                        color: var(--fg-primary);
                        text-transform: none;
                        margin: 0 0 12px;
                    }

                    h3 {
                        font-size: var(--fs-title);
                        line-height: var(--lh-tight);
                        font-weight: var(--fw-semibold);
                        color: var(--fg-primary);
                        text-transform: none;
                        margin: 0 0 12px;
                    }

                    .form-group {
                        margin-bottom: 1.5rem;
                    }

                    label {
                        display: block;
                        font-size: 0.875rem;
                        font-weight: 500;
                        color: hsl(var(--foreground));
                        margin-bottom: 0.5rem;
                    }

                    input, textarea, select {
                        flex: 1;
                        background-color: hsl(var(--background));
                        border: 1px solid hsl(var(--input));
                        border-radius: 0;
                        padding: 0.5rem 0.75rem;
                        font-size: 0.875rem;
                        line-height: 1.25;
                        color: hsl(var(--foreground));
                        transition: border-color 150ms ease-in-out, box-shadow 150ms ease-in-out;
                        width: 100%;
                    }

                    /* Dark mode input field improvements */
                    @media (prefers-color-scheme: dark) {
                        input, textarea, select {
                            background-color: hsl(0 0% 8%);
                            border: 1px solid hsl(0 0% 20%);
                            color: hsl(var(--foreground));
                        }

                        input:focus, textarea:focus, select:focus {
                            background-color: hsl(0 0% 10%);
                            border-color: hsl(var(--ring));
                        }

                        textarea {
                            color: var(--text-primary) !important;
                        }
                    }

                    input:focus, textarea:focus, select:focus {
                        outline: 2px solid transparent;
                        outline-offset: 2px;
                        border-color: hsl(var(--ring));
                        box-shadow: 0 0 0 2px hsl(var(--ring));
                    }

                    input:disabled, textarea:disabled, select:disabled {
                        cursor: not-allowed;
                        opacity: 0.5;
                    }

                    /* Subtle pagination dropdown styling */
                    .per-page-selector {
                        display: flex;
                        align-items: center;
                        gap: 0.5rem;
                        margin: 1rem 0 0 0;
                        padding: 0;
                        background-color: transparent;
                        border: none;
                        border-radius: 0;
                        font-size: 0.875rem;
                    }

                    .per-page-selector label {
                        color: hsl(var(--muted-foreground));
                        font-weight: 500;
                    }

                    .per-page-selector select {
                        background-color: transparent;
                        border: 1px solid hsl(var(--muted));
                        border-radius: 0;
                        padding: 0.25rem 0.5rem;
                        font-size: 0.875rem;
                        color: hsl(var(--muted-foreground));
                        min-width: 50px;
                        cursor: pointer;
                        transition: all 0.2s ease;
                        flex: none;
                        width: auto;
                    }

                    .per-page-selector select:hover {
                        border-color: hsl(var(--ring));
                        background-color: hsl(var(--muted) / 0.5);
                    }

                    .per-page-selector select:focus {
                        outline: 2px solid transparent;
                        outline-offset: 2px;
                        border-color: hsl(var(--ring));
                        box-shadow: 0 0 0 2px hsl(var(--ring) / 0.2);
                    }

                    .per-page-selector span {
                        color: hsl(var(--muted-foreground));
                        font-weight: 500;
                    }

                    /* Form actions layout */
                    .form-actions {
                        display: flex;
                        justify-content: space-between;
                        align-items: center;
                        gap: 1rem;
                        margin-top: 1.5rem;
                        padding-top: 1rem;
                        border-top: 1px solid hsl(var(--border));
                    }

                    .form-actions .button-secondary {
                        order: 1;
                    }

                    .form-actions .button-primary {
                        order: 2;
                    }

                    button {
                        display: inline-flex;
                        align-items: center;
                        justify-content: center;
                        white-space: nowrap;
                        border-radius: 0;
                        font-size: 0.875rem;
                        font-weight: 600;
                        transition: all 150ms ease-in-out;
                        border: 1px solid transparent;
                        cursor: pointer;
                        padding: 0.5rem 1rem;
                        height: 2.25rem;
                        background-color: hsl(var(--primary));
                        color: hsl(var(--primary-foreground));
                    }

                    button:hover {
                        background-color: hsl(var(--primary) / 0.9);
                    }

                    button:focus-visible {
                        outline: 2px solid hsl(var(--ring));
                        outline-offset: 2px;
                    }

                    button:disabled {
                        pointer-events: none;
                        opacity: 0.5;
                    }

                    .button-secondary {
                        background-color: hsl(var(--secondary));
                        color: hsl(var(--secondary-foreground));
                        border: 1px solid hsl(var(--input));
                    }

                    .button-secondary:hover {
                        background-color: hsl(var(--secondary) / 0.8);
                    }

                    .button-outline {
                        border: 1px solid hsl(var(--input));
                        background-color: hsl(var(--background));
                        color: hsl(var(--foreground));
                    }

                    .button-outline:hover {
                        background-color: hsl(var(--accent));
                        color: hsl(var(--accent-foreground));
                    }

                    .button-destructive {
                        background-color: transparent !important;
                        color: #DC2626 !important;
                        border: 1px solid #DC2626 !important;
                    }

                    .button-destructive:hover {
                        background-color: rgba(220, 38, 38, 0.2) !important;
                    }



                    .button-sm {
                        height: 2rem;
                        border-radius: 0;
                        padding: 0 0.75rem;
                        font-size: 0.75rem;
                    }

                    .button-lg {
                        height: 2.75rem;
                        border-radius: 0;
                        padding: 0 2rem;
                        font-size: 1rem;
                    }

                    .grid {
                        display: grid;
                        grid-template-columns: repeat(auto-fit, minmax(320px, 1fr));
                        gap: 1.5rem;
                    }

                    @media (max-width: 640px) {
                        .grid {
                            grid-template-columns: 1fr;
                        }
                    }



                    .info-label,
                    .sub-label,
                    label {
                        font-size: var(--fs-label);
                        line-height: var(--lh-normal);
                        font-weight: var(--fw-medium);
                        color: var(--fg-muted);
                        text-transform: none;
                        letter-spacing: 0.02em;
                        flex-shrink: 0;
                    }

                    .info-value {
                        font-size: 0.875rem;
                        font-family: 'SF Mono', 'Monaco', 'Inconsolata', 'Roboto Mono', 'Courier New', monospace;
                        color: var(--fg-primary);
                        text-align: right;
                        word-break: break-all;
                        overflow-wrap: break-word;
                        hyphens: auto;
                        min-width: 0;
                    }

                    .info-item {
                        display: flex;
                        gap: 0.5rem;
                        align-items: baseline;
                        margin: 8px 0;
                        padding: 1rem 0;
                        border-bottom: 1px solid hsl(var(--border));
                        min-height: 3rem;
                        justify-content: space-between;
                    }

                    .info-item:last-child {
                        border-bottom: none;
                    }

                    /* Card flex spacing improvements */
                    .card-flex {
                        display: flex;
                        gap: 1rem;
                        align-items: center;
                    }

                    .card-flex-content {
                        flex: 1 1 auto;
                    }

                    .card-flex-button {
                        flex: 0 0 auto;
                    }

                    .card-flex-content p {
                        margin: 0 0 12px;
                        line-height: var(--lh-normal);
                    }

                    .card-flex-content p + .card-flex-button,
                    .card-flex-content p + a,
                    .card-flex-content p + button {
                        margin-top: 12px;
                    }

                    .card-flex-content .body + .card-flex-button,
                    .card-flex-content .body + a,
                    .card-flex-content .body + button {
                        margin-top: 12px;
                    }

                    .truncate-value {
                        font-size: 0.875rem;
                        font-family: 'SF Mono', 'Monaco', 'Inconsolata', 'Roboto Mono', 'Courier New', monospace;
                        color: hsl(var(--foreground));
                        text-align: right;
                        overflow: hidden;
                        text-overflow: ellipsis;
                        white-space: nowrap;
                        display: inline-block;
                        max-width: 200px;
                    }

                    .copy-button {
                        background-color: hsl(var(--secondary));
                        color: hsl(var(--secondary-foreground));
                        border: 1px solid hsl(var(--border));
                        border-radius: 0;
                        padding: 0.25rem 0.5rem;
                        cursor: pointer;
                        font-size: 0.75rem;
                        font-weight: 600;
                        margin-left: 0.5rem;
                        transition: all 150ms ease-in-out;
                        height: auto;
                        min-height: auto;
                        flex-shrink: 0;
                    }

                    .copy-button:hover {
                        background-color: hsl(var(--secondary) / 0.8);
                        border-color: hsl(var(--border));
                    }

                    /* Invoice details section */
                    .invoice-details-section {
                        margin-bottom: 1.5rem;
                        padding-bottom: 1.5rem;
                        border-bottom: 1px solid hsl(var(--border));
                    }

                    /* Invoice amount section - prominent display */
                    .invoice-amount-section {
                        text-align: center;
                        margin-bottom: 2rem;
                        padding: 1.5rem;
                        background-color: hsl(var(--muted) / 0.3);
                        border: 1px solid hsl(var(--border));
                        position: relative;
                    }

                    .invoice-amount-section::before,
                    .invoice-amount-section::after {
                        content: '';
                        position: absolute;
                        width: 16px;
                        height: 16px;
                        border: 1px solid hsl(var(--border));
                    }

                    .invoice-amount-section::before {
                        top: -1px;
                        left: -1px;
                        border-right: none;
                        border-bottom: none;
                    }

                    .invoice-amount-section::after {
                        bottom: -1px;
                        right: -1px;
                        border-left: none;
                        border-top: none;
                    }

                    .invoice-amount-label {
                        font-size: 0.875rem;
                        font-weight: 500;
                        color: hsl(var(--muted-foreground));
                        margin-bottom: 0.5rem;
                        text-transform: uppercase;
                        letter-spacing: 0.05em;
                    }

                    .invoice-amount-value {
                        font-size: 2rem;
                        font-weight: 700;
                        font-family: 'SF Mono', 'Monaco', 'Inconsolata', 'Roboto Mono', 'Courier New', monospace;
                        color: hsl(var(--foreground));
                        line-height: 1.2;
                    }

                    /* Invoice display section */
                    .invoice-display-section {
                        margin-top: 1rem;
                    }

                    .invoice-label {
                        font-size: 0.875rem;
                        font-weight: 600;
                        color: hsl(var(--foreground));
                        text-transform: uppercase;
                        letter-spacing: 0.05em;
                        margin-bottom: 0.75rem;
                    }

                    .invoice-display-container {
                        background-color: hsl(var(--muted) / 0.3);
                        border: 1px solid hsl(var(--border));
                        border-radius: 0;
                        padding: 1rem;
                        position: relative;
                    }

                    .invoice-display-container::before,
                    .invoice-display-container::after {
                        content: '';
                        position: absolute;
                        width: 12px;
                        height: 12px;
                        border: 1px solid hsl(var(--border));
                    }

                    .invoice-display-container::before {
                        top: -1px;
                        left: -1px;
                        border-right: none;
                        border-bottom: none;
                    }

                    .invoice-display-container::after {
                        bottom: -1px;
                        right: -1px;
                        border-left: none;
                        border-top: none;
                    }

                    .invoice-textarea {
                        width: 100%;
                        background-color: transparent !important;
                        border: none !important;
                        font-family: 'SF Mono', 'Monaco', 'Inconsolata', 'Roboto Mono', 'Courier New', monospace !important;
                        font-size: 0.875rem !important;
                        color: var(--fg-primary) !important;
                        padding: 0 !important;
                        margin: 0 !important;
                        outline: none !important;
                        word-break: break-all;
                        overflow-wrap: break-word;
                        hyphens: auto;
                        line-height: 1.5;
                        text-align: left;
                        resize: none;
                        min-height: 100px;
                        height: auto;
                        overflow: visible;
                    }

                    .invoice-textarea:focus {
                        box-shadow: none !important;
                        border: none !important;
                    }

                    /* Dark mode invoice display styling */
                    @media (prefers-color-scheme: dark) {
                        .invoice-amount-section {
                            background-color: rgba(255, 255, 255, 0.03) !important;
                            border: none !important;
                        }

                        .invoice-amount-section::before,
                        .invoice-amount-section::after {
                            border-color: rgba(255, 255, 255, 0.2);
                        }

                        .invoice-amount-label {
                            color: var(--text-muted) !important;
                        }

                        .invoice-amount-value {
                            color: var(--text-primary) !important;
                        }

                        .invoice-label {
                            color: var(--text-primary) !important;
                        }

                        .invoice-display-container {
                            background-color: rgba(255, 255, 255, 0.03) !important;
                            border: none !important;
                        }

                        .invoice-display-container::before,
                        .invoice-display-container::after {
                            border-color: rgba(255, 255, 255, 0.2);
                        }

                        .invoice-textarea {
                            color: var(--text-primary) !important;
                        }
                    }

                    /* Responsive invoice display */
                    @media (max-width: 640px) {
                        .invoice-amount-value {
                            font-size: 1.5rem;
                        }

                        .invoice-textarea {
                            font-size: 0.75rem !important;
                            line-height: 1.4;
                            min-height: 80px;
                        }
                    }

                    .balance-item,
                    .balance-item-container {
                        padding: 1.25rem 0;
                        border-bottom: 1px solid hsl(var(--border));
                        margin-bottom: 10px;
                    }

                    .balance-item:last-child,
                    .balance-item-container:last-child {
                        border-bottom: none;
                    }

                    .balance-item .balance-label,
                    .balance-item-container .balance-label,
                    .balance-title,
                    .balance-label {
                        display: block;
                        margin-bottom: 6px;
                        font-size: var(--fs-label);
                        line-height: var(--lh-normal);
                        font-weight: var(--fw-medium);
                        color: var(--fg-muted);
                        letter-spacing: 0.02em;
                        text-transform: none;
                    }

                    .balance-item .balance-amount,
                    .balance-item-container .balance-value,
                    .balance-amount,
                    .balance-amount-value,
                    .balance-value {
                        display: block;
                        font-size: var(--fs-value);
                        line-height: var(--lh-tight);
                        font-weight: var(--fw-bold);
                        color: var(--fg-primary);
                        white-space: nowrap;
                        font-family: 'SF Mono', 'Monaco', 'Inconsolata', 'Roboto Mono', 'Courier New', monospace;
                    }

                    .balance-item .info-label + .info-value,
                    .balance-item .label + .amount,
                    .balance-item-container .info-label + .info-value,
                    .balance-item-container .label + .amount {
                        margin-top: 6px;
                    }

                    .alert {
                        border: 1px solid hsl(var(--border));
                        border-radius: 0;
                        padding: 1rem;
                        margin-bottom: 1rem;
                    }

                    .alert-success {
                        border-color: hsl(142.1 76.2% 36.3%);
                        background-color: hsl(142.1 70.6% 45.3% / 0.1);
                        color: hsl(142.1 76.2% 36.3%);
                    }

                    .alert-destructive {
                        border-color: hsl(var(--destructive));
                        background-color: hsl(var(--destructive) / 0.1);
                        color: hsl(var(--destructive));
                    }

                    .alert-warning {
                        border-color: hsl(32.6 75.4% 55.1%);
                        background-color: hsl(32.6 75.4% 55.1% / 0.1);
                        color: hsl(32.6 75.4% 55.1%);
                    }

                    /* Legacy classes for backward compatibility */
                    .success {
                        border-color: hsl(142.1 76.2% 36.3%);
                        background-color: hsl(142.1 70.6% 45.3% / 0.1);
                        color: hsl(142.1 76.2% 36.3%);
                        border: 1px solid hsl(142.1 76.2% 36.3%);
                        border-radius: 0;
                        padding: 1rem;
                        margin-bottom: 1rem;
                    }

                    .error {
                        border-color: hsl(var(--destructive));
                        background-color: hsl(var(--destructive) / 0.1);
                        color: hsl(var(--destructive));
                        border: 1px solid hsl(var(--destructive));
                        border-radius: 0;
                        padding: 1rem;
                        margin-bottom: 1rem;
                    }

                    .badge {
                        display: inline-flex;
                        align-items: center;
                        border-radius: 9999px;
                        padding: 0.25rem 0.625rem;
                        font-size: 0.75rem;
                        font-weight: 500;
                        line-height: 1;
                        transition: all 150ms ease-in-out;
                        border: 1px solid transparent;
                    }

                    .badge-default {
                        background-color: hsl(var(--primary));
                        color: hsl(var(--primary-foreground));
                    }

                    .badge-secondary {
                        background-color: hsl(var(--secondary));
                        color: hsl(var(--secondary-foreground));
                    }

                    .badge-success {
                        background-color: hsl(142.1 70.6% 45.3%);
                        color: hsl(355.7 78% 98.4%);
                    }

                    .badge-destructive {
                        background-color: hsl(var(--destructive));
                        color: hsl(var(--destructive-foreground));
                    }

                    .badge-outline {
                        background-color: transparent;
                        color: hsl(var(--foreground));
                        border: 1px solid hsl(var(--border));
                    }

                    /* Status badge classes - consistent with payment type badges */
                    .status-badge {
                        display: inline-flex;
                        align-items: center;
                        border-radius: 9999px;
                        padding: 0.25rem 0.625rem;
                        font-size: 0.75rem;
                        font-weight: 500;
                        line-height: 1;
                    }

                    .status-active {
                        background-color: hsl(142.1 70.6% 45.3% / 0.1);
                        color: hsl(142.1 70.6% 45.3%);
                        border: 1px solid hsl(142.1 70.6% 45.3% / 0.2);
                    }

                    .status-inactive {
                        background-color: hsl(0 84.2% 60.2% / 0.1);
                        color: hsl(0 84.2% 60.2%);
                        border: 1px solid hsl(0 84.2% 60.2% / 0.2);
                    }

                    .status-pending {
                        background-color: hsl(215.4 16.3% 46.9% / 0.1);
                        color: hsl(215.4 16.3% 46.9%);
                        border: 1px solid hsl(215.4 16.3% 46.9% / 0.2);
                    }

                    .channel-box {
                        position: relative;
                        background-color: hsl(var(--card));
                        border: 1px solid hsl(var(--border));
                        border-radius: 0;
                        padding: 1.5rem;
                        margin-bottom: 1.5rem;
                    }

                    .channel-box::before,
                    .channel-box::after {
                        content: '';
                        position: absolute;
                        width: 16px;
                        height: 16px;
                        border: 1px solid hsl(var(--border));
                    }

                    .channel-box::before {
                        top: -1px;
                        left: -1px;
                        border-right: none;
                        border-bottom: none;
                    }

                    .channel-box::after {
                        bottom: -1px;
                        right: -1px;
                        border-left: none;
                        border-top: none;
                    }

                    @media (prefers-color-scheme: dark) {
                        .channel-box::before,
                        .channel-box::after {
                            border-color: rgba(255, 255, 255, 0.2);
                        }
                    }

                    .section-header {
                        font-size: 1.25rem;
                        font-weight: 700;
                        color: hsl(var(--foreground));
                        margin-bottom: 1.5rem;
                        line-height: 1.2;
                    }

                    .channel-alias {
                        font-size: 1.25rem;
                        font-weight: 600;
                        color: hsl(var(--foreground));
                        margin-bottom: 1rem;
                        line-height: 1.2;
                    }

                    .channel-details {
                        margin-bottom: 1.5rem;
                    }

                    .detail-row {
                        display: flex;
                        align-items: center;
                        margin-bottom: 1rem;
                        gap: 1.5rem;
                        padding: 0.75rem 0;
                    }

                    .detail-row:last-child {
                        margin-bottom: 0;
                    }

                    .detail-label {
                        font-weight: 500;
                        color: hsl(var(--foreground));
                        opacity: 0.5;
                        font-size: 0.8125rem;
                        min-width: 140px;
                        flex-shrink: 0;
                        letter-spacing: 0.025em;
                        text-transform: uppercase;
                        text-align: right;
                    }

                    .detail-value {
                        color: hsl(var(--foreground));
                        font-family: 'SF Mono', 'Monaco', 'Inconsolata', 'Roboto Mono', 'Courier New', monospace;
                        font-size: 0.875rem;
                        font-weight: 400;
                        word-break: break-all;
                        flex: 1;
                        min-width: 0;
                        letter-spacing: -0.01em;
                        line-height: 1.5;
                    }

                    .detail-value-amount {
                        color: hsl(var(--foreground));
                        font-size: 0.9375rem;
                        font-weight: 500;
                        word-break: break-all;
                        flex: 1;
                        min-width: 0;
                        letter-spacing: 0;
                    }

                    .channel-actions {
                        display: flex;
                        justify-content: space-between;
                        align-items: center;
                        margin-top: 1rem;
                        gap: 1rem;
                    }

                    @media (max-width: 640px) {
                        .channel-actions {
                            flex-direction: column;
                            align-items: stretch;
                        }

                        .detail-row {
                            flex-direction: column;
                            align-items: flex-start;
                            gap: 0.25rem;
                        }

                        .detail-label {
                            min-width: auto;
                        }
                    }

                    .balance-info {
                        display: grid;
                        grid-template-columns: repeat(auto-fit, minmax(120px, 1fr));
                        gap: 1rem;
                        margin-top: 1rem;
                    }

                    @media (max-width: 640px) {
                        .balance-info {
                            grid-template-columns: 1fr;
                        }
                    }

                    .balance-item {
                        position: relative;
                        text-align: center;
                        padding: 1rem;
                        background-color: hsl(var(--muted) / 0.3);
                        border-radius: 0;
                        border: 1px solid hsl(var(--border));
                    }

                    .balance-item::before,
                    .balance-item::after {
                        content: '';
                        position: absolute;
                        width: 12px;
                        height: 12px;
                        border: 1px solid hsl(var(--border));
                    }

                    .balance-item::before {
                        top: -1px;
                        left: -1px;
                        border-right: none;
                        border-bottom: none;
                    }

                    .balance-item::after {
                        bottom: -1px;
                        right: -1px;
                        border-left: none;
                        border-top: none;
                    }

                    @media (prefers-color-scheme: dark) {
                        .balance-item::before,
                        .balance-item::after {
                            border-color: rgba(255, 255, 255, 0.2);
                        }
                    }

                    .balance-amount {
                        font-weight: 600;
                        font-size: 1.125rem;
                        font-family: 'SF Mono', 'Monaco', 'Inconsolata', 'Roboto Mono', 'Courier New', monospace;
                        color: hsl(var(--foreground));
                        line-height: 1.2;
                    }



                    .payment-item {
                        position: relative;
                        background-color: hsl(var(--card));
                        border: 1px solid hsl(var(--border));
                        border-radius: 0;
                        padding: 1.5rem;
                        margin-bottom: 1.5rem;
                    }

                    .payment-item::before,
                    .payment-item::after {
                        content: '';
                        position: absolute;
                        width: 16px;
                        height: 16px;
                        border: 1px solid hsl(var(--border));
                    }

                    .payment-item::before {
                        top: -1px;
                        left: -1px;
                        border-right: none;
                        border-bottom: none;
                    }

                    .payment-item::after {
                        bottom: -1px;
                        right: -1px;
                        border-left: none;
                        border-top: none;
                    }

                    @media (prefers-color-scheme: dark) {
                        .payment-item::before,
                        .payment-item::after {
                            border-color: rgba(255, 255, 255, 0.2);
                        }
                    }

                    /* Dark mode payment card improvements - match other cards */
                    @media (prefers-color-scheme: dark) {
                        .payment-item {
                            background-color: rgba(255, 255, 255, 0.03) !important;
                            border: none !important;
                        }
                    }

                    .payment-header {
                        display: flex;
                        justify-content: space-between;
                        align-items: flex-start;
                        margin-bottom: 1rem;
                        gap: 1rem;
                    }

                    @media (max-width: 640px) {
                        .payment-header {
                            flex-direction: column;
                            align-items: stretch;
                            gap: 0.75rem;
                        }
                    }

                    .payment-direction {
                        display: flex;
                        align-items: center;
                        gap: 0.5rem;
                        font-weight: 600;
                        color: hsl(var(--foreground));
                        flex: 1;
                        min-width: 0;
                    }

                    .direction-icon {
                        font-size: 1.125rem;
                        font-weight: bold;
                        color: hsl(var(--muted-foreground));
                    }

                    .payment-details {
                        display: flex;
                        flex-direction: column;
                        gap: 0.75rem;
                    }

                    .payment-amount {
                        font-size: 1.25rem;
                        font-weight: 600;
                        font-family: 'SF Mono', 'Monaco', 'Inconsolata', 'Roboto Mono', 'Courier New', monospace;
                        color: hsl(var(--foreground));
                        line-height: 1.2;
                    }

                    .payment-info {
                        display: flex;
                        align-items: center;
                        gap: 0.75rem;
                        flex-wrap: wrap;
                    }

                    @media (max-width: 640px) {
                        .payment-info {
                            flex-direction: column;
                            align-items: flex-start;
                            gap: 0.25rem;
                        }
                    }

                    .payment-label {
                        font-weight: 400;
                        color: var(--text-muted);
                        font-size: 0.75rem;
                        flex-shrink: 0;
                        letter-spacing: 0.05em;
                        text-transform: uppercase;
                    }

                    .payment-value {
                        color: var(--text-tertiary);
                        font-family: 'SF Mono', 'Monaco', 'Inconsolata', 'Roboto Mono', 'Courier New', monospace;
                        font-size: 0.8125rem;
                        font-weight: 300;
                        word-break: break-all;
                        min-width: 0;
                        letter-spacing: -0.02em;
                        line-height: 1.7;
                    }

                    .payment-list-header {
                        display: flex;
                        justify-content: space-between;
                        align-items: center;
                        margin-bottom: 1.5rem;
                        padding-bottom: 1rem;
                        border-bottom: 1px solid hsl(var(--border));
                    }

                    @media (max-width: 640px) {
                        .payment-list-header {
                            flex-direction: column;
                            align-items: stretch;
                            gap: 1rem;
                        }
                    }

                    .payment-filter-tabs {
                        display: flex;
                        gap: 0.25rem;
                        overflow-x: auto;
                        -webkit-overflow-scrolling: touch;
                    }

                    .payment-filter-tab {
                        display: inline-flex;
                        align-items: center;
                        justify-content: center;
                        white-space: nowrap;
                        padding: 0.5rem 1rem;
                        border: 1px solid hsl(var(--border));
                        background-color: hsl(var(--background));
                        border-radius: 0;
                        text-decoration: none;
                        color: hsl(var(--muted-foreground));
                        font-size: 0.875rem;
                        font-weight: 600;
                        transition: all 150ms ease-in-out;
                        height: 2.25rem;
                    }

                    .payment-filter-tab:hover {
                        background-color: hsl(var(--accent));
                        color: hsl(var(--accent-foreground));
                        text-decoration: none;
                    }

                    .payment-filter-tab.active {
                        background-color: hsl(var(--primary));
                        color: hsl(var(--primary-foreground));
                        border-color: hsl(var(--primary));
                    }

                    /* Dark mode specific styling for payment filter tabs */
                    @media (prefers-color-scheme: dark) {
                        .payment-filter-tab {
                            background-color: rgba(255, 255, 255, 0.03) !important;
                            border-color: var(--text-muted) !important;
                            color: var(--text-muted) !important;
                        }

                        .payment-filter-tab:hover {
                            background-color: rgba(255, 255, 255, 0.08) !important;
                            color: var(--text-secondary) !important;
                        }

                        .payment-filter-tab.active {
                            background-color: rgba(255, 255, 255, 0.12) !important;
                            color: var(--text-primary) !important;
                            border-color: var(--text-secondary) !important;
                        }

                        .payment-filter-tab.active:hover {
                            background-color: rgba(255, 255, 255, 0.15) !important;
                        }
                    }

                    .payment-type-badge {
                        display: inline-flex;
                        align-items: center;
                        border-radius: 9999px;
                        padding: 0.125rem 0.5rem;
                        font-size: 0.625rem;
                        font-weight: 600;
                        line-height: 1;
                        margin-left: 0.5rem;
                        text-transform: uppercase;
                        letter-spacing: 0.05em;
                    }

                    .payment-type-bolt11 {
                        background-color: hsl(217 91% 60% / 0.1);
                        color: hsl(217 91% 60%);
                        border: 1px solid hsl(217 91% 60% / 0.2);
                    }

                    .payment-type-bolt12 {
                        background-color: hsl(262 83% 58% / 0.1);
                        color: hsl(262 83% 58%);
                        border: 1px solid hsl(262 83% 58% / 0.2);
                    }

                    .payment-type-onchain {
                        background-color: hsl(32 95% 44% / 0.1);
                        color: hsl(32 95% 44%);
                        border: 1px solid hsl(32 95% 44% / 0.2);
                    }

                    /* Dark mode payment type badge improvements */
                    @media (prefers-color-scheme: dark) {
                        .payment-type-onchain {
                            background-color: hsl(32 95% 60% / 0.15);
                            color: hsl(32 95% 70%);
                            border: 1px solid hsl(32 95% 60% / 0.3);
                        }

                        .payment-type-bolt11 {
                            background-color: hsl(217 91% 70% / 0.15);
                            color: hsl(217 91% 80%);
                            border: 1px solid hsl(217 91% 70% / 0.3);
                        }

                        .payment-type-bolt12 {
                            background-color: hsl(262 83% 70% / 0.15);
                            color: hsl(262 83% 80%);
                            border: 1px solid hsl(262 83% 70% / 0.3);
                        }

                        .payment-type-spontaneous {
                            background-color: hsl(142.1 70.6% 60% / 0.15);
                            color: hsl(142.1 70.6% 75%);
                            border: 1px solid hsl(142.1 70.6% 60% / 0.3);
                        }

                        .payment-type-bolt11-jit {
                            background-color: hsl(199 89% 65% / 0.15);
                            color: hsl(199 89% 80%);
                            border: 1px solid hsl(199 89% 65% / 0.3);
                        }
                    }

                    .payment-type-spontaneous {
                        background-color: hsl(142.1 70.6% 45.3% / 0.1);
                        color: hsl(142.1 70.6% 45.3%);
                        border: 1px solid hsl(142.1 70.6% 45.3% / 0.2);
                    }

                    .payment-type-bolt11-jit {
                        background-color: hsl(199 89% 48% / 0.1);
                        color: hsl(199 89% 48%);
                        border: 1px solid hsl(199 89% 48% / 0.2);
                    }

                    .payment-type-unknown {
                        background-color: hsl(var(--muted));
                        color: hsl(var(--muted-foreground));
                        border: 1px solid hsl(var(--border));
                    }

                    /* Pagination */
                    .pagination-controls {
                        display: flex;
                        justify-content: center;
                        align-items: center;
                        margin: 2rem 0;
                    }

                    .pagination {
                        display: flex;
                        align-items: center;
                        gap: 0.25rem;
                        list-style: none;
                    }

                    .pagination-btn, .pagination-number {
                        display: inline-flex;
                        align-items: center;
                        justify-content: center;
                        white-space: nowrap;
                        border-radius: 0;
                        font-size: 0.875rem;
                        font-weight: 600;
                        transition: all 150ms ease-in-out;
                        border: 1px solid hsl(var(--border));
                        background-color: hsl(var(--background));
                        color: hsl(var(--foreground));
                        text-decoration: none;
                        cursor: pointer;
                        height: 2.25rem;
                        min-width: 2.25rem;
                        padding: 0 0.5rem;
                    }

                    .pagination-btn:hover, .pagination-number:hover {
                        background-color: hsl(var(--accent));
                        color: hsl(var(--accent-foreground));
                        text-decoration: none;
                    }

                    .pagination-number.active {
                        background-color: hsl(var(--primary));
                        color: hsl(var(--primary-foreground));
                        border-color: hsl(var(--primary));
                    }

                    .pagination-btn.disabled {
                        background-color: hsl(var(--muted));
                        color: hsl(var(--muted-foreground));
                        cursor: not-allowed;
                        opacity: 0.5;
                        pointer-events: none;
                    }

                    .pagination-ellipsis {
                        display: flex;
                        align-items: center;
                        justify-content: center;
                        height: 2.25rem;
                        width: 2.25rem;
                        color: hsl(var(--muted-foreground));
                        font-size: 0.875rem;
                    }

                    /* Responsive adjustments */
                    @media (max-width: 640px) {
                        .container {
                            padding: 0 1rem;
                        }

                        header {
                            padding: 1rem 0;
                            margin-bottom: 1rem;
                        }

                        h1 {
                            font-size: 1.5rem;
                        }

                        nav ul {
                            flex-wrap: wrap;
                        }

                        .card {
                            padding: 1rem;
                            margin-bottom: 1rem;
                        }

                        .info-item {
                            flex-direction: column;
                            align-items: flex-start;
                            gap: 0.75rem;
                            padding: 1rem 0;
                            min-height: auto;
                        }

                        .info-value, .truncate-value {
                            text-align: left;
                            max-width: 100%;
                        }

                        .copy-button {
                            margin-left: 0;
                            margin-top: 0.25rem;
                            align-self: flex-start;
                        }

                        .balance-amount-value {
                            font-size: 1.25rem;
                        }

                        .pagination {
                            flex-wrap: wrap;
                            justify-content: center;
                            gap: 0.125rem;
                        }

                        .pagination-btn, .pagination-number {
                            height: 2rem;
                            min-width: 2rem;
                            font-size: 0.75rem;
                        }
                    }

                    /* Node Information Section Styling */
                    .node-info-section {
                        display: flex;
                        gap: 1.5rem;
                        margin-bottom: 1.5rem;
                        align-items: stretch;
                    }

                    .node-info-main-container {
                        position: relative;
                        flex: 1;
                        display: flex;
                        flex-direction: column;
                        gap: 1rem;
                        background-color: hsl(var(--card));
                        border: 1px solid hsl(var(--border));
                        border-radius: 0;
                        padding: 1.5rem;
                        box-shadow: none;
                        height: 100%;
                    }

                    .node-info-main-container::before,
                    .node-info-main-container::after {
                        content: '';
                        position: absolute;
                        width: 16px;
                        height: 16px;
                        border: 1px solid hsl(var(--border));
                    }

                    .node-info-main-container::before {
                        top: -1px;
                        left: -1px;
                        border-right: none;
                        border-bottom: none;
                    }

                    .node-info-main-container::after {
                        bottom: -1px;
                        right: -1px;
                        border-left: none;
                        border-top: none;
                    }

                    @media (prefers-color-scheme: dark) {
                        .node-info-main-container::before,
                        .node-info-main-container::after {
                            border-color: rgba(255, 255, 255, 0.2);
                        }
                    }

                    .node-info-left {
                        display: flex;
                        align-items: center;
                        gap: 1rem;
                        margin-bottom: 1rem;
                    }

                    .node-avatar {
                        flex-shrink: 0;
                        background-color: hsl(var(--muted) / 0.3);
                        border: 1px solid hsl(var(--border));
                        border-radius: 0;
                        padding: 0.75rem;
                        display: flex;
                        align-items: center;
                        justify-content: center;
                        width: 80px;
                        height: 80px;
                    }

                    .avatar-image {
                        width: 48px;
                        height: 48px;
                        border-radius: 0;
                        object-fit: cover;
                        display: block;
                    }

                    .node-details {
                        flex: 1;
                        min-width: 0;
                    }

                    .node-name {
                        font-size: var(--fs-title);
                        font-weight: var(--fw-semibold);
                        color: var(--fg-primary);
                        margin: 0 0 0.25rem 0;
                        line-height: var(--lh-tight);
                        word-wrap: break-word;
                        overflow-wrap: break-word;
                        hyphens: auto;
                    }

                    .node-address {
                        font-size: 0.75rem;
                        color: var(--text-muted);
                        font-weight: 500;
                        letter-spacing: 0.05em;
                        text-transform: uppercase;
                        margin: 0;
                        line-height: var(--lh-normal);
                    }

                    .node-content-box {
                        background-color: hsl(var(--muted) / 0.3);
                        border: 1px solid hsl(var(--border));
                        border-radius: 0;
                        min-height: 200px;
                        padding: 1rem;
                        display: flex;
                        align-items: center;
                        justify-content: center;
                        color: hsl(var(--muted-foreground));
                        overflow: hidden;
                    }

                    .node-metrics {
                        flex-shrink: 0;
                        width: 280px;
                        display: flex;
                        flex-direction: column;
                        align-self: stretch;
                    }

                    .node-metrics .card {
                        margin-bottom: 0;
                        flex: 1;
                        display: flex;
                        flex-direction: column;
                        align-self: stretch;
                    }

                    .node-metrics .metrics-container {
                        flex-direction: column;
                        margin: 1rem 0 0 0;
                        flex: 1;
                        display: flex;
                        justify-content: flex-start;
                        gap: 1rem;
                        align-items: stretch;
                    }

                    .node-metrics .metric-card {
                        min-width: auto;
                        padding: 1rem;
                        height: fit-content;
                        display: flex;
                        flex-direction: column;
                        align-items: center;
                        justify-content: center;
                        text-align: center;
                    }

                    /* Mobile responsive design for node info */
                    @media (max-width: 768px) {
                        .node-info-section {
                            flex-direction: column;
                            gap: 1rem;
                        }

                        .node-info-left {
                            flex-direction: column;
                            align-items: flex-start;
                            text-align: center;
                            gap: 0.75rem;
                        }

                        .node-avatar {
                            align-self: center;
                        }

                        .node-details {
                            text-align: center;
                            width: 100%;
                        }

                        .node-content-box {
                            min-height: 150px;
                            padding: 1rem;
                        }

                        .node-metrics {
                            width: 100%;
                        }

                        .node-metrics .metrics-container {
                            flex-direction: row;
                            flex-wrap: wrap;
                        }

                        .node-metrics .metric-card {
                            flex: 1;
                            min-width: 120px;
                        }
                    }

                    @media (max-width: 480px) {
                        .node-info-left {
                            gap: 0.5rem;
                        }

                        .node-avatar {
                            width: 64px;
                            height: 64px;
                            padding: 0.5rem;
                        }

                        .avatar-image {
                            width: 40px;
                            height: 40px;
                        }

                        .node-name {
                            font-size: 1rem;
                            word-wrap: break-word;
                            overflow-wrap: break-word;
                            hyphens: auto;
                        }

                        .node-address {
                            font-size: 0.8125rem;
                        }

                        .node-content-box {
                            min-height: 120px;
                            padding: 0.75rem;
                        }

                        .node-metrics .metrics-container {
                            flex-direction: column;
                            gap: 0.75rem;
                        }
                    }

                    /* Activity Grid Layout - Side by Side */
                    .activity-grid {
                        display: grid;
                        grid-template-columns: 1fr 1fr;
                        gap: 0;
                        margin-top: 1.5rem;
                    }

                    .activity-section {
                        padding: 2rem 1.5rem;
                        border-right: 1px solid hsl(var(--border));
                        border-top: 1px solid hsl(var(--border));
                    }

                    .activity-section:last-child {
                        border-right: none;
                    }

                    .activity-header {
                        display: flex;
                        align-items: center;
                        gap: 0.75rem;
                        margin-bottom: 2rem;
                        padding-bottom: 0;
                        border-bottom: none;
                    }

                    .activity-icon-box {
                        flex-shrink: 0;
                        background-color: hsl(var(--muted) / 0.3);
                        border: 1px solid hsl(var(--border));
                        border-radius: 0;
                        padding: 0.5rem;
                        display: flex;
                        align-items: center;
                        justify-content: center;
                        width: 36px;
                        height: 36px;
                    }

                    .activity-icon-box svg {
                        color: hsl(var(--foreground));
                    }

                    .activity-title {
                        font-size: 1rem;
                        font-weight: 400;
                        color: hsl(var(--foreground));
                        margin: 0;
                        text-transform: none;
                        letter-spacing: normal;
                    }

                    .activity-metrics {
                        display: flex;
                        flex-direction: column;
                        gap: 1rem;
                    }

                    .activity-metric-card {
                        position: relative;
                        text-align: left;
                        padding: 1rem;
                        background-color: hsl(var(--muted) / 0.3);
                        border-radius: 0;
                        border: 1px solid hsl(var(--border));
                    }

                    .activity-metric-card::before,
                    .activity-metric-card::after {
                        content: '';
                        position: absolute;
                        width: 12px;
                        height: 12px;
                        border: 1px solid hsl(var(--border));
                    }

                    .activity-metric-card::before {
                        top: -1px;
                        left: -1px;
                        border-right: none;
                        border-bottom: none;
                    }

                    .activity-metric-card::after {
                        bottom: -1px;
                        right: -1px;
                        border-left: none;
                        border-top: none;
                    }

                    .activity-metric-label {
                        display: block;
                        margin-bottom: 0.5rem;
                        font-size: 0.875rem;
                        font-weight: 400;
                        color: hsl(var(--muted-foreground));
                        text-transform: none;
                        letter-spacing: normal;
                    }

                    .activity-metric-value {
                        display: block;
                        font-size: 1.5rem;
                        font-weight: 600;
                        color: hsl(var(--foreground));
                        font-family: 'SF Mono', 'Monaco', 'Inconsolata', 'Roboto Mono', 'Courier New', monospace;
                        line-height: 1.2;
                    }

                    /* Dark mode activity styling */
                    @media (prefers-color-scheme: dark) {
                        .activity-icon-box {
                            background-color: rgba(255, 255, 255, 0.03) !important;
                            border: none !important;
                        }

                        .activity-icon-box svg {
                            color: var(--text-primary);
                        }

                        .activity-title {
                            color: var(--text-primary);
                        }

                        .activity-metric-card {
                            background-color: rgba(255, 255, 255, 0.03) !important;
                            border: none !important;
                        }

                        .activity-metric-card::before,
                        .activity-metric-card::after {
                            border-color: rgba(255, 255, 255, 0.2);
                        }

                        .activity-metric-label {
                            color: var(--text-muted) !important;
                        }

                        .activity-metric-value {
                            color: var(--text-primary) !important;
                        }
                    }

                    /* Responsive activity grid */
                    @media (max-width: 768px) {
                        .activity-grid {
                            grid-template-columns: 1fr;
                        }

                        .activity-section {
                            border-right: none;
                            border-bottom: 1px solid hsl(var(--border));
                            padding: 1.5rem 1rem;
                        }

                        .activity-section:last-child {
                            border-bottom: none;
                        }
                    }

                    /* Responsive typography adjustments */
                    @media (max-width: 640px) {
                        :root {
                            --fs-value: 1.45rem;
                        }

                        .node-name {
                            font-size: 0.875rem;
                        }

                        .activity-metric-value {
                            font-size: 1.25rem;
                        }
                    }

                    /* Payment tabs styling */
                    .payment-tabs {
                        display: flex;
                        gap: 0.5rem;
                        margin-bottom: 1.5rem;
                        border-bottom: 1px solid hsl(var(--border));
                    }

                    .payment-tab {
                        display: inline-flex;
                        align-items: center;
                        justify-content: center;
                        white-space: nowrap;
                        padding: 0.75rem 1.5rem;
                        border: none;
                        border-bottom: 2px solid transparent;
                        background-color: transparent;
                        border-radius: 0;
                        text-decoration: none;
                        color: hsl(var(--muted-foreground));
                        font-size: 0.9375rem;
                        font-weight: 600;
                        transition: all 200ms ease;
                        cursor: pointer;
                        position: relative;
                        margin-bottom: -1px;
                    }

                    .payment-tab:hover {
                        color: hsl(var(--foreground));
                        background-color: hsl(var(--muted) / 0.5);
                    }

                    .payment-tab.active {
                        color: hsl(var(--foreground));
                        border-bottom-color: hsl(var(--foreground));
                        background-color: transparent;
                    }

                    /* Dark mode tab styling */
                    @media (prefers-color-scheme: dark) {
                        .payment-tab {
                            color: var(--text-muted);
                        }

                        .payment-tab:hover {
                            color: var(--text-secondary);
                            background-color: rgba(255, 255, 255, 0.05);
                        }

                        .payment-tab.active {
                            color: var(--text-primary);
                            border-bottom-color: var(--text-primary);
                        }
                    }

                    /* Tab content */
                    .tab-content {
                        display: none;
                        animation: fade-in 0.2s ease-out;
                    }

                    .tab-content.active {
                        display: block;
                    }

                    @keyframes fade-in {
                        from {
                            opacity: 0;
                            transform: translateY(4px);
                        }
                        to {
                            opacity: 1;
                            transform: translateY(0);
                        }
                    }

                    @media (max-width: 480px) {
                        .node-name {
                            font-size: 0.8125rem;
                        }
                    }

                    /* Dark mode adjustments for globe animation */
                    @media (prefers-color-scheme: dark) {
                        .node-content-box .world {
                            border-color: rgba(156, 163, 175, 0.4);
                            fill: rgba(156, 163, 175, 0.2);
                        }
                    }

                    /* Address display styling */
                    .address-display {
                        margin: 1.5rem 0;
                    }

                    .address-container {
                        padding: 1rem 0;
                    }

                    .address-text {
                        font-family: 'SF Mono', 'Monaco', 'Inconsolata', 'Roboto Mono', 'Courier New', monospace;
                        font-size: 1.25rem;
                        font-weight: 500;
                        color: hsl(var(--foreground));
                        word-break: break-all;
                        overflow-wrap: break-word;
                        hyphens: auto;
                        flex: 1;
                        min-width: 0;
                        line-height: 1.4;
                        background-color: transparent;
                        border: none;
                        padding: 0;
                    }


                    /* Dark mode address styling */
                    @media (prefers-color-scheme: dark) {
                        .address-text {
                            color: var(--text-primary) !important;
                        }
                    }

                    /* Responsive address display */
                    @media (max-width: 640px) {
                        .address-text {
                            font-size: 1.125rem;
                            text-align: center;
                        }
                    }

                    /* Transaction confirmation styling */
                    .transaction-details {
                        margin-top: 1rem;
                    }

                    .transaction-details .detail-row {
                        display: flex;
                        align-items: baseline;
                        margin-bottom: 1rem;
                        gap: 1rem;
                        padding: 0.75rem 0;
                        border-bottom: 1px solid hsl(var(--border));
                    }

                    .transaction-details .detail-row:last-child {
                        border-bottom: none;
                        margin-bottom: 0;
                    }

                    .transaction-details .detail-label {
                        font-weight: 400;
                        color: var(--text-muted);
                        font-size: 0.75rem;
                        min-width: 180px;
                        flex-shrink: 0;
                        letter-spacing: 0.05em;
                        text-transform: uppercase;
                    }

                    .transaction-details .detail-value {
                        color: var(--text-tertiary);
                        font-family: 'SF Mono', 'Monaco', 'Inconsolata', 'Roboto Mono', 'Courier New', monospace;
                        font-size: 0.8125rem;
                        font-weight: 300;
                        word-break: break-all;
                        flex: 1;
                        min-width: 0;
                        letter-spacing: -0.02em;
                        line-height: 1.7;
                    }

                    .transaction-details .detail-value-amount {
                        color: var(--text-secondary);
                        font-family: 'SF Mono', 'Monaco', 'Inconsolata', 'Roboto Mono', 'Courier New', monospace;
                        font-size: 1rem;
                        font-weight: 600;
                        letter-spacing: 0;
                        flex: 1;
                        min-width: 0;
                    }

                    .send-all-notice {
                        border: 1px solid hsl(32.6 75.4% 55.1%);
                        background-color: hsl(32.6 75.4% 55.1% / 0.1);
                    }

                    .send-all-notice h3 {
                        color: hsl(32.6 75.4% 55.1%);
                        font-size: 1rem;
                        font-weight: 600;
                        margin-bottom: 0.5rem;
                    }

                    .send-all-notice p {
                        color: hsl(32.6 75.4% 55.1%);
                        font-size: 0.875rem;
                        line-height: 1.4;
                        margin: 0;
                    }

                    /* Dark mode transaction styling */
                    @media (prefers-color-scheme: dark) {
                        .transaction-details .detail-label {
                            color: var(--text-muted) !important;
                        }

                        .transaction-details .detail-value,
                        .transaction-details .detail-value-amount {
                            color: var(--text-primary) !important;
                        }

                        .send-all-notice {
                            background-color: hsl(32.6 75.4% 55.1% / 0.15) !important;
                            border-color: hsl(32.6 75.4% 55.1% / 0.3) !important;
                        }
                    }

                    /* Responsive transaction details */
                    @media (max-width: 640px) {
                        .transaction-details .detail-row {
                            flex-direction: column;
                            align-items: flex-start;
                            gap: 0.5rem;
                        }

                        .transaction-details .detail-label {
                            min-width: auto;
                            font-size: 0.8125rem;
                        }

                        .transaction-details .detail-value,
                        .transaction-details .detail-value-amount {
                            font-size: 0.875rem;
                        }
                    }
                    "
                }
            }
            body {
                header {
                    div class="container" {
                        div class="header-content" {
                            div class="header-left" {
                                div class="header-avatar" {
                                    img src="/static/images/nut.png" alt="CDK LDK Node Icon" class="header-avatar-image";
                                }
                                div class="node-info" {
                                    div class="node-status" {
                                        @if is_running {
                                            span class="status-indicator" {}
                                            span class="status-text" { "Running" }
                                        } @else {
                                            span class="status-indicator status-inactive" {}
                                            span class="status-text status-inactive" { "Inactive" }
                                        }
                                    }
                                    h1 class="node-title" { "CDK LDK Node" }
                                    span class="node-subtitle" { "Cashu Mint & Lightning Network Node Management" }
                                }
                            }
                            div class="header-right" {
                                // Right side content can be added here later if needed
                            }
                        }
                    }
                }

                nav {
                    div class="container" {
                        ul {
                            li {
                                a href="/" {
                                    svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.25" stroke-linecap="round" stroke-linejoin="round" style="margin-right: 0.5rem;" {
                                        path d="M15.6 2.7a10 10 0 1 0 5.7 5.7" {}
                                        circle cx="12" cy="12" r="2" {}
                                        path d="M13.4 10.6 19 5" {}
                                    }
                                    "Dashboard"
                                }
                            }
                            li {
                                a href="/balance" {
                                    svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.25" stroke-linecap="round" stroke-linejoin="round" style="margin-right: 0.5rem;" {
                                        path d="M4 14a1 1 0 0 1-.78-1.63l9.9-10.2a.5.5 0 0 1 .86.46l-1.92 6.02A1 1 0 0 0 13 10h7a1 1 0 0 1 .78 1.63l-9.9 10.2a.5.5 0 0 1-.86-.46l1.92-6.02A1 1 0 0 0 11 14z" {}
                                    }
                                    "Lightning"
                                }
                            }
                            li {
                                a href="/onchain" {
                                    svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.25" stroke-linecap="round" stroke-linejoin="round" style="margin-right: 0.5rem;" {
                                        path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71" {}
                                        path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71" {}
                                    }
                                    "On-chain"
                                }
                            }
                            li {
                                a href="/payments" {
                                    svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.25" stroke-linecap="round" stroke-linejoin="round" style="margin-right: 0.5rem;" {
                                        path d="M12 18H4a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h16a2 2 0 0 1 2 2v5" {}
                                        path d="m16 19 3 3 3-3" {}
                                        path d="M18 12h.01" {}
                                        path d="M19 16v6" {}
                                        path d="M6 12h.01" {}
                                        circle cx="12" cy="12" r="2" {}
                                    }
                                    "All Payments"
                                }
                            }
                        }
                    }
                }

                main class="container" {
                    (content)
                }



            }
        }
    }
}
