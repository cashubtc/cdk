use maud::{html, Markup, DOCTYPE};

pub fn layout(title: &str, content: Markup) -> Markup {
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
                        --radius: 0.5rem;

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
                            color: var(--text-muted) !important;
                        }

                        .detail-value, .detail-value-amount {
                            color: var(--text-secondary) !important;
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
                            border-radius: 0.75rem !important;
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
                        border-radius: var(--radius);
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
                        border-radius: calc(var(--radius) - 2px);
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

                    .status-text {
                        font-size: 0.875rem;
                        font-weight: 500;
                        color: #10b981;
                    }

                    .node-title {
                        font-size: 1.875rem;
                        font-weight: 600;
                        color: var(--header-title);
                        margin: 0;
                        line-height: 1.1;
                    }

                    .node-subtitle {
                        font-size: 0.875rem;
                        color: var(--header-subtitle);
                        font-weight: 500;
                    }

                    .header-right {
                        display: flex;
                        align-items: center;
                    }



                    /* Responsive header */
                    @media (max-width: 768px) {
                        .header-content {
                            flex-direction: column;
                            gap: 1rem;
                            text-align: center;
                        }

                        .header-left {
                            flex-direction: column;
                            text-align: center;
                        }

                        .node-title {
                            font-size: 1.5rem;
                        }
                    }

                        nav a {
                            color: var(--text-muted) !important;
                        }

                        nav a:hover {
                            color: var(--text-secondary) !important;
                            background-color: rgba(255, 255, 255, 0.05) !important;
                        }

                        nav a.active {
                            color: var(--text-primary) !important;
                            background-color: rgba(255, 255, 255, 0.08) !important;
                        }

                        nav a.active:hover {
                            background-color: rgba(255, 255, 255, 0.1) !important;
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
                        background-image: url('/static/images/bg.jpg?v=3');
                        background-size: cover;
                        background-position: center;
                        background-repeat: no-repeat;
                        border-bottom: 1px solid hsl(var(--border));
                        margin-bottom: 2rem;
                        text-align: left;
                        width: 100%;
                        height: 200px; /* Reduced height for more compact header */
                        display: flex;
                        align-items: center;
                        justify-content: flex-start;
                    }

                    /* Dark mode header background - using different image */
                    @media (prefers-color-scheme: dark) {
                        header {
                            background-image: url('/static/images/bg-dark.jpg?v=3');
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

                    @media (max-width: 768px) {
                        header {
                            height: 150px; /* Smaller height on mobile */
                        }

                        header .container {
                            padding: 0 1rem;
                        }

                        h1 {
                            font-size: 2.25rem;
                        }

                        .subtitle {
                            font-size: 1.1rem;
                        }
                    }

                    /* Card fade-in animation */
                    @keyframes fade-in {
                        from { opacity: 0; transform: translateY(10px); }
                        to { opacity: 1; transform: translateY(0); }
                    }

                    .card {
                        animation: fade-in 0.3s ease-out;
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
                        border-radius: calc(var(--radius) - 2px);
                        transition: all 200ms cubic-bezier(0.4, 0, 0.2, 1);
                        position: relative;
                        min-height: 3rem;
                    }

                    nav a:hover {
                        color: hsl(var(--foreground));
                        background-color: hsl(var(--muted));
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
                        background-color: hsl(var(--card));
                        border: 1px solid hsl(var(--border));
                        border-radius: var(--radius);
                        padding: 1.5rem;
                        margin-bottom: 1.5rem;
                        box-shadow: 0 1px 2px 0 rgba(0, 0, 0, 0.05);
                    }

                    /* Metric cards styling - matching balance-item style */
                    .metrics-container {
                        display: flex;
                        gap: 1rem;
                        margin: 1rem 0;
                        flex-wrap: wrap;
                    }

                    .metric-card {
                        flex: 1;
                        min-width: 200px;
                        text-align: center;
                        padding: 1rem;
                        background-color: hsl(var(--muted) / 0.3);
                        border-radius: calc(var(--radius) - 2px);
                        border: 1px solid hsl(var(--border));
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
                        border-radius: calc(var(--radius) - 2px);
                        padding: 0.5rem 0.75rem;
                        font-size: 0.875rem;
                        line-height: 1.25;
                        color: hsl(var(--foreground));
                        transition: border-color 150ms ease-in-out, box-shadow 150ms ease-in-out;
                        width: 100%;
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

                    button {
                        display: inline-flex;
                        align-items: center;
                        justify-content: center;
                        white-space: nowrap;
                        border-radius: calc(var(--radius) - 2px);
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
                        border-radius: calc(var(--radius) - 4px);
                        padding: 0 0.75rem;
                        font-size: 0.75rem;
                    }

                    .button-lg {
                        height: 2.75rem;
                        border-radius: var(--radius);
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
                        border-radius: calc(var(--radius) - 4px);
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
                        border-radius: var(--radius);
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
                        border-radius: var(--radius);
                        padding: 1rem;
                        margin-bottom: 1rem;
                    }

                    .error {
                        border-color: hsl(var(--destructive));
                        background-color: hsl(var(--destructive) / 0.1);
                        color: hsl(var(--destructive));
                        border: 1px solid hsl(var(--destructive));
                        border-radius: var(--radius);
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
                        background-color: hsl(var(--card));
                        border: 1px solid hsl(var(--border));
                        border-radius: var(--radius);
                        padding: 1.5rem;
                        margin-bottom: 1.5rem;
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
                        align-items: baseline;
                        margin-bottom: 0.75rem;
                        gap: 1rem;
                    }

                    .detail-row:last-child {
                        margin-bottom: 0;
                    }

                    .detail-label {
                        font-weight: 500;
                        color: hsl(var(--muted-foreground));
                        font-size: 0.875rem;
                        min-width: 120px;
                        flex-shrink: 0;
                    }

                    .detail-value {
                        color: hsl(var(--foreground));
                        font-family: 'SF Mono', 'Monaco', 'Inconsolata', 'Roboto Mono', 'Courier New', monospace;
                        font-size: 0.875rem;
                        word-break: break-all;
                        flex: 1;
                        min-width: 0;
                    }

                    .detail-value-amount {
                        color: hsl(var(--foreground));
                        font-size: 0.875rem;
                        word-break: break-all;
                        flex: 1;
                        min-width: 0;
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
                        text-align: center;
                        padding: 1rem;
                        background-color: hsl(var(--muted) / 0.3);
                        border-radius: calc(var(--radius) - 2px);
                        border: 1px solid hsl(var(--border));
                    }

                    .balance-amount {
                        font-weight: 600;
                        font-size: 1.125rem;
                        font-family: 'SF Mono', 'Monaco', 'Inconsolata', 'Roboto Mono', 'Courier New', monospace;
                        color: hsl(var(--foreground));
                        line-height: 1.2;
                    }



                    .payment-item {
                        background-color: hsl(var(--card));
                        border: 1px solid hsl(var(--border));
                        border-radius: var(--radius);
                        padding: 1.5rem;
                        margin-bottom: 1.5rem;
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
                        font-weight: 500;
                        color: hsl(var(--muted-foreground));
                        font-size: 0.875rem;
                        flex-shrink: 0;
                    }

                    .payment-value {
                        color: hsl(var(--foreground));
                        font-family: 'SF Mono', 'Monaco', 'Inconsolata', 'Roboto Mono', 'Courier New', monospace;
                        font-size: 0.875rem;
                        word-break: break-all;
                        min-width: 0;
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
                        border-radius: calc(var(--radius) - 2px);
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
                        border-radius: calc(var(--radius) - 2px);
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
                        flex: 1;
                        display: flex;
                        flex-direction: column;
                        gap: 1rem;
                        background-color: hsl(var(--card));
                        border: 1px solid hsl(var(--border));
                        border-radius: var(--radius);
                        padding: 1.5rem;
                        box-shadow: 0 1px 2px 0 rgba(0, 0, 0, 0.05);
                        height: 100%;
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
                        border-radius: var(--radius);
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
                        border-radius: calc(var(--radius) - 2px);
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
                        font-size: 0.875rem;
                        color: var(--fg-muted);
                        margin: 0;
                        line-height: var(--lh-normal);
                    }

                    .node-content-box {
                        background-color: hsl(var(--muted) / 0.3);
                        border: 1px solid hsl(var(--border));
                        border-radius: var(--radius);
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

                    /* Responsive typography adjustments */
                    @media (max-width: 640px) {
                        :root {
                            --fs-value: 1.45rem;
                        }

                        .node-name {
                            font-size: 0.875rem;
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
                                        span class="status-indicator status-running" {}
                                        span class="status-text" { "Running" }
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
