pub struct ChartDefinition {
    pub id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub period: &'static str,
}

pub const BILLBOARD_ICON: &str = "https://www.billboard.com/wp-content/themes/vip/pmc-billboard-2021/assets/app/icons/icon-512x512.png";

pub const CHARTS: &[ChartDefinition] = &[
    ChartDefinition {
        id: "hot-100",
        title: "Billboard Hot 100",
        description: "Top songs in the US across streaming, sales, and radio play.",
        period: "Weekly",
    },
    ChartDefinition {
        id: "billboard-200",
        title: "Billboard 200",
        description: "Top albums in the US across sales and equivalent units.",
        period: "Weekly",
    },
    ChartDefinition {
        id: "streaming-songs",
        title: "Streaming Songs",
        description: "Most streamed songs in the US.",
        period: "Weekly",
    },
    ChartDefinition {
        id: "digital-song-sales",
        title: "Digital Song Sales",
        description: "Top-selling digital tracks.",
        period: "Weekly",
    },
    ChartDefinition {
        id: "radio-songs",
        title: "Radio Songs",
        description: "Songs with the highest radio audience impressions.",
        period: "Weekly",
    },
    ChartDefinition {
        id: "top-album-sales",
        title: "Top Album Sales",
        description: "Top-selling albums by pure sales.",
        period: "Weekly",
    },
    ChartDefinition {
        id: "current-albums",
        title: "Current Albums",
        description: "Best-performing current albums.",
        period: "Weekly",
    },
    ChartDefinition {
        id: "independent-albums",
        title: "Independent Albums",
        description: "Top albums from independent labels.",
        period: "Weekly",
    },
    ChartDefinition {
        id: "catalog-albums",
        title: "Catalog Albums",
        description: "Top-performing catalog albums.",
        period: "Weekly",
    },
    ChartDefinition {
        id: "soundtracks",
        title: "Soundtracks",
        description: "Top soundtrack albums.",
        period: "Weekly",
    },
    ChartDefinition {
        id: "vinyl-albums",
        title: "Vinyl Albums",
        description: "Top-selling vinyl albums.",
        period: "Weekly",
    },
    ChartDefinition {
        id: "world-albums",
        title: "World Albums",
        description: "Top world music albums.",
        period: "Weekly",
    },
    ChartDefinition {
        id: "canadian-hot-100",
        title: "Canadian Hot 100",
        description: "Top songs in Canada.",
        period: "Weekly",
    },
    ChartDefinition {
        id: "india-songs-hotw",
        title: "India Songs",
        description: "Top songs in India.",
        period: "Weekly",
    },
    ChartDefinition {
        id: "billboard-global-200",
        title: "Billboard Global 200",
        description: "Top songs globally.",
        period: "Weekly",
    },
];

pub fn find_chart(chart_id: &str) -> Option<&'static ChartDefinition> {
    CHARTS.iter().find(|chart| chart.id == chart_id)
}

pub fn chart_url(chart_id: &str) -> String {
    format!("https://www.billboard.com/charts/{chart_id}/")
}

pub fn is_album_chart(chart_id: &str) -> bool {
    matches!(
        chart_id,
        "billboard-200"
            | "top-album-sales"
            | "current-albums"
            | "independent-albums"
            | "catalog-albums"
            | "soundtracks"
            | "vinyl-albums"
            | "world-albums"
    )
}
