mod settings;

use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, TimeZone, Utc};
use rusqlite::{Connection, Result, Row};
use serde::Serialize;
use settings::SETTINGS;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::process::Command;
use tera::{Context, Tera};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize)]
pub struct Highlight {
    pub id: String,
    pub parent_id: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Paper {
    pub id: String,
    pub has_url: bool,
    // roam_ref is either the full URL if there is one, or a ref in the format @zotero_<id>
    pub roam_ref: String,
    pub source_url: String,
    pub zotero_url: String,
    pub title: String,
    pub author: String,
    pub saved_at: DateTime<Utc>,
    pub published_date: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize)]
struct HighlightJson {
    id: String,
    content: String,
    note: String,
    note_saved_at: String,
}

fn parse_date(date_str: &str) -> Option<DateTime<Utc>> {
    if date_str.is_empty() {
        return None;
    }

    // Try to parse the date in format YYYY-MM-DD
    if let Ok(naive_date) = NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
        let naive_datetime = naive_date.and_hms_opt(0, 0, 0).unwrap();
        return Some(Utc.from_utc_datetime(&naive_datetime));
    }

    // Try to parse the date in format YYYY-MM-DD HH:MM:SS
    if let Ok(naive_datetime) = NaiveDateTime::parse_from_str(date_str, "%Y-%m-%d %H:%M:%S") {
        return Some(Utc.from_utc_datetime(&naive_datetime));
    }

    None
}

fn map_row_to_paper(row: &Row) -> Result<Paper> {
    let paper_id_int: i64 = row.get(0)?;
    let paper_id = paper_id_int.to_string();
    let title: String = row.get(1)?;
    let url: Option<String> = row.get(2)?;
    let date_added: String = row.get(3)?;
    let zotero_uri: String = row.get(4)?;
    let publication_date: Option<String> = row.get(5)?;
    let authors: Option<String> = row.get(6)?;

    let has_url = url.is_some() && !url.as_ref().unwrap().is_empty();
    let source_url = url.unwrap_or_default();

    let roam_ref = if has_url {
        source_url.clone()
    } else {
        format!("@zotero_{}", paper_id)
    };

    let saved_at = parse_date(&date_added).unwrap_or_else(|| Utc::now());
    let published_date = publication_date.and_then(|date| parse_date(&date));

    Ok(Paper {
        id: paper_id,
        has_url,
        roam_ref,
        source_url,
        zotero_url: zotero_uri,
        title,
        author: authors.unwrap_or_default(),
        saved_at,
        published_date,
    })
}

fn query_papers(conn: &Connection) -> Result<Vec<Paper>> {
    let query = r#"
    SELECT DISTINCT
        papers.itemID AS paperID,
        title_values.value AS title,
        url_values.value AS url,
        SUBSTR(papers.dateAdded, 1, 10) as dateAdded,
        'zotero://select/items/' ||
            CASE WHEN papers.libraryID = 1 THEN '0' ELSE papers.libraryID END ||
            '_' || papers.key AS zotero_uri,
        SUBSTR(date_values.value, 1, 10) AS publication_date,
        (
            SELECT GROUP_CONCAT(author_name, ', ')
            FROM (
                SELECT DISTINCT
                    CASE
                        WHEN c.fieldMode = 1 THEN c.lastName
                        ELSE
                            CASE
                                WHEN c.firstName IS NOT NULL AND c.firstName != ''
                                THEN c.firstName || ' ' || c.lastName
                                ELSE c.lastName
                            END
                    END AS author_name,
                    ic.orderIndex
                FROM
                    itemCreators ic
                JOIN
                    creators c ON ic.creatorID = c.creatorID
                WHERE
                    ic.itemID = papers.itemID
                ORDER BY
                    ic.orderIndex
            )
        ) AS authors
    FROM
        items AS papers
    JOIN
        itemData AS title_data ON papers.itemID = title_data.itemID AND title_data.fieldID = 1
    JOIN
        itemDataValues AS title_values ON title_data.valueID = title_values.valueID
    LEFT JOIN
        itemData AS url_data ON papers.itemID = url_data.itemID AND url_data.fieldID = 13
    LEFT JOIN
        itemDataValues AS url_values ON url_data.valueID = url_values.valueID
    LEFT JOIN
        itemData AS date_data ON papers.itemID = date_data.itemID AND date_data.fieldID = 6
    LEFT JOIN
        itemDataValues AS date_values ON date_data.valueID = date_values.valueID
    JOIN
        itemAttachments AS attachments ON papers.itemID = attachments.parentItemID
    GROUP BY
        papers.itemID, title_values.value, url_values.value, papers.libraryID, papers.key, date_values.value
    "#;

    let mut stmt = conn.prepare(query)?;
    let paper_iter = stmt.query_map([], |row| map_row_to_paper(row))?;

    let mut papers = Vec::new();
    for paper_result in paper_iter {
        papers.push(paper_result?);
    }

    Ok(papers)
}

fn query_highlights(conn: &Connection) -> Result<HashMap<String, Vec<HighlightJson>>> {
    let query = r#"
    SELECT
        annotations.itemID AS annotationID,
        annotations.text AS highlight_text,
        annotations.comment AS highlight_comment,
        attachments.parentItemID AS paperID,
        SUBSTR(items.dateAdded, 1, 10) AS date_added
    FROM
        itemAnnotations AS annotations
    JOIN
        itemAttachments AS attachments ON annotations.parentItemID = attachments.itemID
    JOIN
        items ON annotations.itemID = items.itemID
    ORDER BY
        attachments.parentItemID,
        CAST(SUBSTR(annotations.sortIndex, 1, 5) AS INTEGER),
        CAST(SUBSTR(annotations.sortIndex, 7, 6) AS INTEGER),
        CAST(SUBSTR(annotations.sortIndex, 14) AS INTEGER)
    "#;

    let mut stmt = conn.prepare(query)?;
    let mut rows = stmt.query([])?;

    let mut highlights_map: HashMap<String, Vec<HighlightJson>> = HashMap::new();

    while let Some(row) = rows.next()? {
        let annotation_id_int: i64 = row.get(0)?;
        let annotation_id = annotation_id_int.to_string();
        let highlight_text: Option<String> = row.get(1)?;
        let highlight_comment: Option<String> = row.get(2)?;
        let paper_id_int: i64 = row.get(3)?;
        let paper_id = paper_id_int.to_string();
        let date_added: String = row.get(4)?;

        if highlight_text.is_none() || highlight_text.as_ref().unwrap().trim().is_empty() {
            continue;
        }

        let highlight_json = HighlightJson {
            id: annotation_id,
            content: highlight_text.unwrap_or_default(),
            note: highlight_comment.unwrap_or_default(),
            note_saved_at: date_added,
        };

        highlights_map
            .entry(paper_id)
            .or_insert_with(Vec::new)
            .push(highlight_json);
    }

    Ok(highlights_map)
}

fn get_existing_refs(
    org_roam_dir: &Path,
) -> Result<HashMap<String, String>, Box<dyn std::error::Error>> {
    let output = Command::new("rg")
        .args([
            "--with-filename",
            "--fixed-strings",
            ":ROAM_REFS:",
            &org_roam_dir.to_string_lossy(),
        ])
        .output()?;

    if !output.status.success() {
        eprintln!(
            "ripgrep command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        return Ok(HashMap::new());
    }

    let output_str = String::from_utf8(output.stdout)?;
    let mut refs_map = HashMap::new();
    for line in output_str.lines() {
        if let Some((filename, rest)) = line.split_once(":") {
            if let Some(roam_ref) = rest.strip_prefix(":ROAM_REFS:") {
                let trimmed_ref = roam_ref.trim().to_string();
                if !trimmed_ref.is_empty() {
                    refs_map.insert(trimmed_ref, filename.to_string());
                }
            }
        }
    }
    Ok(refs_map)
}

fn get_new_entry_filename(org_roam_dir: &Path, title: &str, url: Option<&str>) -> String {
    let now = Local::now();
    let slug = slug::slugify(title);
    let truncated_slug = if slug.len() > 100 {
        slug[..100].to_string()
    } else {
        slug
    };

    let maybe_url_part = if let Some(u) = url {
        if !u.is_empty() {
            let hash = md5::compute(u);
            let hash_str = format!("{:x}", hash);
            let truncated_hash = &hash_str[..8];
            format!("-{}", truncated_hash)
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    org_roam_dir
        .join(format!(
            "{}-{}{}.org",
            now.format("%Y%m%d%H%M%S"),
            truncated_slug,
            maybe_url_part
        ))
        .to_string_lossy()
        .into_owned()
}

fn get_duplicate_titles(documents: &[Paper]) -> Vec<String> {
    let mut title_counts: HashMap<String, u32> = HashMap::new();
    for document in documents {
        *title_counts.entry(document.title.clone()).or_default() += 1;
    }
    title_counts
        .into_iter()
        .filter(|(_, count)| *count > 1)
        .map(|(title, _)| title)
        .collect()
}

fn generate_highlight_content(
    highlights_with_notes: &[HighlightJson],
    tera: &Tera,
) -> Result<String, tera::Error> {
    if highlights_with_notes.is_empty() {
        return Ok(String::new());
    }
    let mut highlight_context = Context::new();
    highlight_context.insert("highlights", highlights_with_notes);
    tera.render("highlights.tera", &highlight_context)
}

fn generate_file_content(
    document: &Paper,
    highlight_content: &str,
    tera: &Tera,
) -> Result<String, tera::Error> {
    let uuid = Uuid::new_v4().to_string();

    let mut context = Context::new();
    context.insert("uuid", &uuid);
    context.insert("roam_ref", &document.roam_ref);
    if document.has_url {
        context.insert("full_url", &document.source_url);
    }
    context.insert("zotero_url", &document.zotero_url);
    context.insert("title", &document.title);
    context.insert("author", &document.author);
    context.insert(
        "saved_at",
        &document.saved_at.format("%Y-%m-%d").to_string(),
    );
    if let Some(published_date) = document.published_date {
        context.insert(
            "published_date",
            &published_date.format("%Y-%m-%d").to_string(),
        );
    }
    context.insert("highlight_content", highlight_content);
    tera.render("document.org.tera", &context)
}

fn edit_file(
    filename: &str,
    _parent: &Paper,
    highlight_content: &str,
) -> Result<(), std::io::Error> {
    let content = fs::read_to_string(filename)?;
    let lines: Vec<&str> = content.lines().collect();

    let highlight_marker = "* zotero:highlights";
    let highlight_index = lines
        .iter()
        .position(|line| line.trim() == highlight_marker)
        .unwrap_or(lines.len());

    let mut new_content = lines[..highlight_index].join("\n");

    if !new_content.is_empty() || !highlight_content.is_empty() {
        new_content.push('\n');
    }
    new_content.push_str(highlight_content);

    fs::write(filename, new_content)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let start_time = std::time::Instant::now();

    let tera = Tera::new(&SETTINGS.templates_dir.to_string_lossy())?;

    let org_roam_dir = Path::new(&SETTINGS.org_roam_dir);
    if !org_roam_dir.is_dir() {
        eprintln!("Org roam directory not found: {}", org_roam_dir.display());
        return Err(format!("Org roam directory not found: {}", org_roam_dir.display()).into());
    }

    let conn = Connection::open(&SETTINGS.zotero_db_path)?;

    println!("Scanning {:?} for existing refs...", org_roam_dir);
    let existing_refs = get_existing_refs(org_roam_dir)?;
    println!("Found {} existing org-roam refs.", existing_refs.len());

    println!("Querying papers from Zotero DB...");
    let papers = query_papers(&conn)?;
    println!("Found {} papers with potential attachments.", papers.len());
    if papers.is_empty() {
        println!("No papers found. Exiting.");
        return Ok(());
    }

    println!("Querying highlights from Zotero DB...");
    let highlights_map = query_highlights(&conn)?;
    println!("Found highlights for {} papers.", highlights_map.len());

    let duplicate_titles = get_duplicate_titles(&papers);
    if !duplicate_titles.is_empty() {
        println!("Found duplicate titles: {:?}", duplicate_titles);
    }

    let mut files_created = 0;
    let mut files_edited = 0;

    println!("Processing papers and generating/updating org files...");
    for paper in &papers {
        let current_highlights = highlights_map.get(&paper.id).cloned().unwrap_or_default();

        let highlight_content_str = generate_highlight_content(&current_highlights, &tera)?;

        if let Some(filename) = existing_refs.get(&paper.roam_ref) {
            match edit_file(filename, paper, &highlight_content_str) {
                Ok(_) => {
                    println!("Edited file: {}", filename);
                    files_edited += 1;
                }
                Err(e) => eprintln!("Error editing file {}: {}", filename, e),
            }
        } else {
            let filename = if duplicate_titles.contains(&paper.title) {
                get_new_entry_filename(
                    org_roam_dir,
                    &paper.title,
                    if paper.has_url {
                        Some(&paper.source_url)
                    } else {
                        None
                    },
                )
            } else {
                get_new_entry_filename(org_roam_dir, &paper.title, None)
            };

            match generate_file_content(paper, &highlight_content_str, &tera) {
                Ok(content) => match fs::write(&filename, &content) {
                    Ok(_) => {
                        println!("Created file: {}", filename);
                        files_created += 1;
                    }
                    Err(e) => eprintln!("Error writing file {}: {}", filename, e),
                },
                Err(e) => eprintln!("Error generating content for {}: {}", paper.title, e),
            }
        }
    }

    println!("\n--- Summary ---");
    println!("Files created: {}", files_created);
    println!("Files edited: {}", files_edited);
    let duration = start_time.elapsed();
    println!("Total time taken: {:?}", duration);

    Ok(())
}
