//! Class strings for the shared controls, taken from shadcn/ui's new-york-v4 components
//! (https://ui.shadcn.com), the reference Dioxus Components follows. Compose a base with one
//! variant and one size, e.g. `class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_SM}"`.
//!
//! Every string is a literal so Tailwind's source scan picks the classes up.

/// Button base. Add one `BUTTON_*` variant and one size.
pub const BUTTON: &str = "inline-flex shrink-0 cursor-pointer items-center justify-center gap-2 rounded-md text-sm font-medium whitespace-nowrap transition-all outline-none focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50 disabled:pointer-events-none disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4";
pub const BUTTON_PRIMARY: &str = "bg-primary text-primary-foreground hover:bg-primary/90";
pub const BUTTON_DESTRUCTIVE: &str = "bg-destructive text-white hover:bg-destructive/90 focus-visible:ring-destructive/20 dark:bg-destructive/60 dark:focus-visible:ring-destructive/40";
pub const BUTTON_OUTLINE: &str = "border bg-background shadow-xs hover:bg-accent hover:text-accent-foreground dark:border-input dark:bg-input/30 dark:hover:bg-input/50";
pub const BUTTON_SECONDARY: &str = "bg-secondary text-secondary-foreground hover:bg-secondary/80";
pub const BUTTON_GHOST: &str =
    "hover:bg-accent hover:text-accent-foreground dark:hover:bg-accent/50";
pub const BUTTON_LINK: &str = "text-primary underline-offset-4 hover:underline";
/// Discord's "Blurple" brand fill, for the "Join Discord" CTAs. Brand color per
/// https://discord.com/branding; left unmixed with the theme since it marks a third-party link.
pub const BUTTON_DISCORD: &str = "bg-[#5865F2] text-white hover:bg-[#4752C4]";
pub const BUTTON_MD: &str = "h-9 px-4 py-2 has-[>svg]:px-3";
pub const BUTTON_SM: &str = "h-8 gap-1.5 px-3 has-[>svg]:px-2.5";
pub const BUTTON_XS: &str =
    "h-6 gap-1 px-2 text-xs has-[>svg]:px-1.5 [&_svg:not([class*='size-'])]:size-3";
pub const BUTTON_LG: &str = "h-10 px-6 has-[>svg]:px-4";
pub const BUTTON_ICON: &str = "size-9";

/// Text input, search field and native `select`.
pub const INPUT: &str = "h-9 w-full min-w-0 rounded-md border border-input bg-transparent px-3 py-1 text-base shadow-xs transition-[color,box-shadow] outline-none selection:bg-primary selection:text-primary-foreground placeholder:text-muted-foreground disabled:pointer-events-none disabled:cursor-not-allowed disabled:opacity-50 md:text-sm dark:bg-input/30 focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50 aria-invalid:border-destructive aria-invalid:ring-destructive/20 dark:aria-invalid:ring-destructive/40";
pub const TEXTAREA: &str = "flex field-sizing-content min-h-16 w-full rounded-md border border-input bg-transparent px-3 py-2 text-base shadow-xs transition-[color,box-shadow] outline-none placeholder:text-muted-foreground focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50 disabled:cursor-not-allowed disabled:opacity-50 aria-invalid:border-destructive aria-invalid:ring-destructive/20 md:text-sm dark:bg-input/30 dark:aria-invalid:ring-destructive/40";
pub const LABEL: &str = "flex items-center gap-2 text-sm leading-none font-medium select-none peer-disabled:cursor-not-allowed peer-disabled:opacity-50";

/// Badge base. Add one `BADGE_*` variant.
pub const BADGE: &str = "inline-flex w-fit shrink-0 items-center justify-center gap-1 overflow-hidden rounded-full border border-transparent px-2 py-0.5 text-xs font-medium whitespace-nowrap transition-[color,box-shadow] [&>svg]:pointer-events-none [&>svg]:size-3";
pub const BADGE_PRIMARY: &str = "bg-primary text-primary-foreground [a&]:hover:bg-primary/90";
pub const BADGE_SECONDARY: &str =
    "bg-secondary text-secondary-foreground [a&]:hover:bg-secondary/90";
pub const BADGE_DESTRUCTIVE: &str = "bg-destructive/10 text-destructive dark:bg-destructive/20";
pub const BADGE_OUTLINE: &str =
    "border-border! text-foreground [a&]:hover:bg-accent [a&]:hover:text-accent-foreground";
pub const BADGE_SUCCESS: &str = "bg-success/10 text-success dark:bg-success/20";
pub const BADGE_WARNING: &str = "bg-warning/10 text-warning dark:bg-warning/20";

/// Card surface. `CARD` has shadcn's vertical rhythm (children pad themselves with `px-6`);
/// `CARD_SURFACE` is the bare surface for cards that set their own padding.
pub const CARD: &str =
    "flex flex-col gap-6 rounded-xl border bg-card py-6 text-card-foreground shadow-sm";
pub const CARD_SURFACE: &str = "rounded-xl border bg-card text-card-foreground shadow-sm";

/// Data table parts.
pub const TABLE_WRAP: &str = "relative w-full overflow-x-auto rounded-xl border";
pub const TABLE: &str = "w-full caption-bottom text-sm";
pub const TABLE_HEAD_ROW: &str = "border-b bg-muted/50";
pub const TABLE_ROW: &str = "border-b transition-colors last:border-0 hover:bg-muted/50";
pub const TABLE_TH: &str =
    "h-10 px-3 text-left align-middle font-medium whitespace-nowrap text-muted-foreground";
/// Header cell for a numeric, right-aligned column.
pub const TABLE_TH_NUM: &str =
    "h-10 px-3 text-right align-middle font-medium whitespace-nowrap text-muted-foreground";
pub const TABLE_TD: &str = "p-3 align-middle";

/// Segmented tabs (also used for filter and window switches made of links).
pub const TABS_LIST: &str = "inline-flex h-9 w-fit items-center justify-center rounded-lg bg-muted p-[3px] text-muted-foreground";
pub const TAB: &str = "inline-flex h-[calc(100%-1px)] flex-1 items-center justify-center gap-1.5 rounded-md border border-transparent px-3 py-1 text-sm font-medium whitespace-nowrap text-foreground/60 transition-all hover:text-foreground focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50 dark:text-muted-foreground dark:hover:text-foreground";
pub const TAB_ACTIVE: &str =
    "bg-background text-foreground! shadow-sm dark:border-input dark:bg-input/30";

/// Popover / dropdown surface and its items.
pub const POPOVER: &str = "z-50 rounded-md border bg-popover p-1 text-popover-foreground shadow-md";
pub const MENU_ITEM: &str = "flex w-full cursor-pointer items-center gap-2 rounded-sm px-2 py-1.5 text-left text-sm outline-hidden select-none hover:bg-accent hover:text-accent-foreground focus-visible:bg-accent";

/// Inline link inside running text.
pub const TEXT_LINK: &str = "font-medium underline underline-offset-4 hover:text-foreground";

/// Page title (`h1`), section title (`h2`) and lead paragraph.
pub const H1: &str = "text-3xl font-semibold tracking-tight text-balance";
pub const H2: &str = "text-xl font-semibold tracking-tight";
pub const LEAD: &str = "text-muted-foreground text-balance";
