mod settings;

use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};
use rusqlite::{Connection, Result, Row};
use serde_json::{json, Value};
use settings::SETTINGS;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct Highlight {
    pub id: String,
    pub parent_id: String,
    pub content: String,
}

#[derive(Debug, Clone)]
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

fn query_highlights(conn: &Connection) -> Result<HashMap<String, Vec<Value>>> {
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

    let mut highlights_map: HashMap<String, Vec<Value>> = HashMap::new();

    while let Some(row) = rows.next()? {
        let annotation_id_int: i64 = row.get(0)?;
        let annotation_id = annotation_id_int.to_string();
        let highlight_text: String = row.get(1)?;
        let highlight_comment: Option<String> = row.get(2)?;
        let paper_id_int: i64 = row.get(3)?;
        let paper_id = paper_id_int.to_string();
        let date_added: String = row.get(4)?;

        let highlight_json = json!({
            "id": annotation_id,
            "content": highlight_text,
            "note": highlight_comment.unwrap_or_default(),
            "note_saved_at": date_added
        });

        highlights_map
            .entry(paper_id)
            .or_insert_with(Vec::new)
            .push(highlight_json);
    }

    Ok(highlights_map)
}

fn main() -> Result<()> {
    // Connect to your Zotero SQLite database
    let conn = Connection::open(&SETTINGS.zotero_db_path)?;

    // Query papers
    let papers = query_papers(&conn)?;
    println!("Found {} papers", papers.len());

    // Query highlights and build the hashmap
    let highlights_map = query_highlights(&conn)?;
    println!("Found highlights for {} papers", highlights_map.len());

    // Example: Access papers and their highlights
    for paper in &papers {
        println!("Paper: {} by {}", paper.title, paper.author);

        if let Some(highlights) = highlights_map.get(&paper.id) {
            println!("  Highlights: {}", highlights.len());
            for (i, highlight) in highlights.iter().enumerate().take(2) {
                println!(
                    "  {}. {}",
                    i + 1,
                    highlight["content"].as_str().unwrap_or("[No content]")
                );
            }
            if highlights.len() > 2 {
                println!("  ... and {} more", highlights.len() - 2);
            }
        }
    }

    Ok(())
}
