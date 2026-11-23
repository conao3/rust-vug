use super::{MIB, number, string, strings, tables, unknown_keys, valid_account};

#[derive(Debug)]
pub(super) struct Layout {
    pub esp_bytes: u64,
    pub esp_name: String,
    pub root_name: String,
    pub esp_options: String,
}

impl Layout {
    pub fn load(table: &toml::Table) -> Result<Option<Self>, String> {
        let Some(disk) = table.get("disk") else {
            return if table.contains_key("filesystem") {
                Err("filesystem: requires disk".into())
            } else {
                Ok(None)
            };
        };
        let disk = disk.as_table().ok_or("disk: expected a table")?;
        unknown_keys(disk, &["table", "partition"], "disk.")?;
        if string(disk, "table", "disk.")? != "gpt" {
            return Err("disk.table: expected gpt".into());
        }
        let partitions = tables(disk, "partition")?;
        let [esp, root] = partitions.as_slice() else {
            return Err("disk.partition: expected ESP followed by root".into());
        };
        unknown_keys(esp, &["name", "type", "size-mib"], "disk.partition.")?;
        unknown_keys(root, &["name", "type", "grow"], "disk.partition.")?;
        let esp_name = string(esp, "name", "disk.partition.")?;
        let root_name = string(root, "name", "disk.partition.")?;
        if [&esp_name, &root_name]
            .iter()
            .any(|name| name.len() > 36 || !valid_account(name))
            || esp_name == root_name
        {
            return Err("disk.partition: expected distinct partition names".into());
        }
        let esp_bytes = u64::from(number(esp, "size-mib", "disk.partition.")?) * MIB;
        if string(esp, "type", "disk.partition.")? != "esp"
            || string(root, "type", "disk.partition.")? != "linux-filesystem"
            || root.get("grow").and_then(toml::Value::as_bool) != Some(true)
            || esp_bytes < 64 * MIB
        {
            return Err("disk.partition: expected ESP of at least 64 MiB and growing root".into());
        }
        let mut esp_options = None;
        let mut have_root = false;
        for filesystem in tables(table, "filesystem")? {
            unknown_keys(
                filesystem,
                &["partition", "mount", "type", "options"],
                "filesystem.",
            )?;
            let partition = string(filesystem, "partition", "filesystem.")?;
            let mount = string(filesystem, "mount", "filesystem.")?;
            let kind = string(filesystem, "type", "filesystem.")?;
            match (mount.as_str(), kind.as_str()) {
                ("/", "ext4") if partition == root_name && !have_root => {
                    if filesystem.contains_key("options") {
                        return Err("filesystem.options: unsupported for root".into());
                    }
                    have_root = true;
                }
                ("/boot", "vfat") if partition == esp_name && esp_options.is_none() => {
                    let options = if filesystem.contains_key("options") {
                        strings(filesystem, "options", "filesystem.")?
                    } else {
                        Vec::new()
                    };
                    if options.iter().any(|option| {
                        option.is_empty()
                            || !option
                                .bytes()
                                .all(|b| b.is_ascii_alphanumeric() || b"=_-.".contains(&b))
                    }) {
                        return Err("filesystem.options: invalid mount option".into());
                    }
                    esp_options = Some(if options.is_empty() {
                        "defaults".into()
                    } else {
                        options.join(",")
                    });
                }
                _ => return Err("filesystem: expected one ext4 root and one vfat ESP".into()),
            }
        }
        match (have_root, esp_options) {
            (true, Some(esp_options)) => Ok(Some(Self {
                esp_name,
                esp_bytes,
                root_name,
                esp_options,
            })),
            _ => Err("filesystem: root and ESP mounts are required".into()),
        }
    }

    pub fn partition_spec(&self) -> String {
        format!(
            "{} {} {} {}\n",
            self.esp_name, self.esp_bytes, self.root_name, self.esp_options
        )
    }
}
