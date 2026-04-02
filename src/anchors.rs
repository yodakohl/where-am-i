#[derive(Debug, Clone, Copy)]
pub struct Anchor {
    pub id: &'static str,
    pub host: &'static str,
    pub metro: &'static str,
    pub latitude: f64,
    pub longitude: f64,
}

impl Anchor {
    pub fn label(&self) -> String {
        format!("{} ({})", self.metro, self.id)
    }
}

pub const BUILTIN_ANCHORS: &[Anchor] = &[
    Anchor {
        id: "eu-central-1",
        host: "ec2.eu-central-1.amazonaws.com",
        metro: "Frankfurt",
        latitude: 50.1109,
        longitude: 8.6821,
    },
    Anchor {
        id: "eu-south-1",
        host: "ec2.eu-south-1.amazonaws.com",
        metro: "Milan",
        latitude: 45.4642,
        longitude: 9.1900,
    },
    Anchor {
        id: "eu-west-1",
        host: "ec2.eu-west-1.amazonaws.com",
        metro: "Dublin",
        latitude: 53.3498,
        longitude: -6.2603,
    },
    Anchor {
        id: "eu-north-1",
        host: "ec2.eu-north-1.amazonaws.com",
        metro: "Stockholm",
        latitude: 59.3293,
        longitude: 18.0686,
    },
    Anchor {
        id: "us-east-1",
        host: "ec2.us-east-1.amazonaws.com",
        metro: "Ashburn",
        latitude: 39.0438,
        longitude: -77.4874,
    },
    Anchor {
        id: "ca-central-1",
        host: "ec2.ca-central-1.amazonaws.com",
        metro: "Montreal",
        latitude: 45.5017,
        longitude: -73.5673,
    },
    Anchor {
        id: "us-west-2",
        host: "ec2.us-west-2.amazonaws.com",
        metro: "Portland",
        latitude: 45.5152,
        longitude: -122.6784,
    },
    Anchor {
        id: "ap-south-1",
        host: "ec2.ap-south-1.amazonaws.com",
        metro: "Mumbai",
        latitude: 19.0760,
        longitude: 72.8777,
    },
    Anchor {
        id: "ap-southeast-1",
        host: "ec2.ap-southeast-1.amazonaws.com",
        metro: "Singapore",
        latitude: 1.3521,
        longitude: 103.8198,
    },
    Anchor {
        id: "ap-southeast-2",
        host: "ec2.ap-southeast-2.amazonaws.com",
        metro: "Sydney",
        latitude: -33.8688,
        longitude: 151.2093,
    },
    Anchor {
        id: "ap-east-1",
        host: "ec2.ap-east-1.amazonaws.com",
        metro: "Hong Kong",
        latitude: 22.3193,
        longitude: 114.1694,
    },
    Anchor {
        id: "ap-northeast-1",
        host: "ec2.ap-northeast-1.amazonaws.com",
        metro: "Tokyo",
        latitude: 35.6762,
        longitude: 139.6503,
    },
    Anchor {
        id: "sa-east-1",
        host: "ec2.sa-east-1.amazonaws.com",
        metro: "Sao Paulo",
        latitude: -23.5505,
        longitude: -46.6333,
    },
    Anchor {
        id: "af-south-1",
        host: "ec2.af-south-1.amazonaws.com",
        metro: "Cape Town",
        latitude: -33.9249,
        longitude: 18.4241,
    },
];
