// SPDX-FileCopyrightText: OpenTalk GmbH <mail@opentalk.eu>
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use chrono::{DateTime, Datelike};
use parse_zoneinfo::line::{DaySpec, Line, LineParser, Weekday, Year};
use parse_zoneinfo::table::{RuleInfo, Saving, Table, TableBuilder};
use std::env::var;
use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;

const FMT_ICS_LOCAL: &str = "%Y%m%dT%H%M%S";

// function mostly taken from
// https://github.com/chronotope/chrono-tz/blob/main/chrono-tz-build/src/lib.rs
fn build_table() -> Table {
    fn strip_comments(mut line: String) -> String {
        if let Some(pos) = line.find('#') {
            line.truncate(pos);
        };
        line
    }

    let tzfiles = [
        "tz/africa",
        "tz/antarctica",
        "tz/asia",
        "tz/australasia",
        "tz/backward",
        "tz/etcetera",
        "tz/europe",
        "tz/northamerica",
        "tz/southamerica",
    ];

    let lines = tzfiles
        .iter()
        .map(Path::new)
        .map(|path| {
            File::open(path).unwrap_or_else(|e| panic!("cannot open {}: {}", path.display(), e))
        })
        .map(BufReader::new)
        .flat_map(BufRead::lines)
        .map(Result::unwrap)
        .map(strip_comments);

    let parser = LineParser::new();
    let mut table = TableBuilder::new();

    for line in lines {
        match parser.parse_str(&line).unwrap() {
            Line::Zone(zone) => table.add_zone_line(zone).unwrap(),
            Line::Continuation(cont) => table.add_continuation_line(cont).unwrap(),
            Line::Rule(rule) => table.add_rule_line(rule).unwrap(),
            Line::Link(link) => table.add_link_line(link).unwrap(),
            Line::Space => {}
        }
    }

    table.build()
}

fn main() -> Result<(), io::Error> {
    let table = build_table();

    let mut librs = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(Path::new(&var("OUT_DIR").unwrap()).join("lib.rs"))?;

    writeln!(
        librs,
        r#"
    use chrono_tz::Tz;
    use ics::{{TimeZone, Standard, Daylight}};
    use ics::properties::RRule;

    pub trait ToIcsTimeZone {{
        fn to_latest_ics_timezone(&self) -> TimeZone;
    }}
    
    impl ToIcsTimeZone for Tz {{
        fn to_latest_ics_timezone(&self) -> TimeZone {{
            match *self {{
    "#
    )?;

    for zone in table.zonesets.keys().chain(table.links.keys()) {
        let zone_ident = convert_bad_chars(zone);
        writeln!(librs, "Tz::{} => {{", zone_ident)?;

        // only generate def for last zoneset
        let mut zoneset = table.get_zoneset(zone).unwrap().iter().rev();

        let zoneinfo = zoneset.next().unwrap();

        let dtstart = if let Some(previous) = zoneset.next() {
            DateTime::from_timestamp(previous.end_time.unwrap().to_timestamp(), 0)
        } else {
            // DTSTART fallback
            DateTime::from_timestamp(0, 0)
        }
        .unwrap();

        match &zoneinfo.saving {
            Saving::NoSaving => {
                writeln!(
                    librs,
                    r#"let standard = Standard::new("{}", "{}", "{}");"#,
                    dtstart.format(FMT_ICS_LOCAL),
                    format_utc_offset(zoneinfo.offset),
                    format_utc_offset(zoneinfo.offset)
                )?;
                writeln!(librs, r#"TimeZone::standard("{zone}", standard)"#)?;
            }
            Saving::OneOff(_offset) => {
                // unclear how to implement if it isn't used anywhere
                unimplemented!(
                    "Saving::OneOff wasn't used until now. \
                    But now {} is using it and it needs to be implemented",
                    zone
                );
            }
            Saving::Multiple(rule_name) => {
                // VTIMEZONE has two subcomponents STANDARD AND DAYLIGHT(DST)
                //
                // The tz-db has rules which define the standard and DST but doesn't mark it as such.
                // Figure which rules are active by only looking at rules with the TO field set to 'max'
                //
                // Panic if there's more than 2 active rules, because that doesn't seem to be the
                // case right now anywhere and I'm not sure how one would handle that.
                //
                // If there's fewer than two we just fall back to the standard component.
                //
                // If there's 2 rules applying for a zone figure out which one is the DST one
                // by looking at the additional offset specified.

                let rules: Vec<_> = table.rulesets[rule_name]
                    .iter()
                    .filter(|rule| matches!(rule.to_year, Some(Year::Maximum)))
                    .collect();

                assert!(rules.len() <= 2, "{rule_name} has more than 2 active rules");

                match rules[..] {
                    [] => {
                        writeln!(
                            librs,
                            r#"let standard = Standard::new("{}", "{}", "{}");"#,
                            dtstart.format(FMT_ICS_LOCAL),
                            format_utc_offset(zoneinfo.offset),
                            format_utc_offset(zoneinfo.offset)
                        )?;
                        writeln!(librs, r#"TimeZone::standard("{zone}", standard)"#)?;
                    }
                    [_rule] => {
                        unimplemented!("only 1 active rule")
                    }
                    [rule1, rule2] => {
                        let (standard_rule, dst_rule) = if rule1.time_to_add == 0 {
                            assert!(rule2.time_to_add != 0);

                            (rule1, rule2)
                        } else {
                            assert!(rule1.time_to_add != 0);
                            assert!(rule2.time_to_add == 0);

                            (rule2, rule1)
                        };

                        let utc_offset = zoneinfo.offset;
                        let dst_offset = zoneinfo.offset + dst_rule.time_to_add;

                        let standard_dtstart = DateTime::from_timestamp(
                            standard_rule.absolute_datetime(
                                dtstart.year() as i64,
                                utc_offset,
                                dst_offset,
                            ),
                            0,
                        )
                        .unwrap();

                        let dst_dtstart = DateTime::from_timestamp(
                            dst_rule.absolute_datetime(
                                dtstart.year() as i64,
                                utc_offset,
                                dst_offset,
                            ),
                            0,
                        )
                        .unwrap();

                        let standard_rrule = rule_to_rrule(standard_rule);
                        let dst_rrule = rule_to_rrule(dst_rule);

                        writeln!(
                            librs,
                            r#"
                            let mut standard = Standard::new("{}", "{}", "{}");
                            standard.push(RRule::new("{standard_rrule}"));

                            let mut daylight = Daylight::new("{}", "{}", "{}");
                            daylight.push(RRule::new("{dst_rrule}"));

                            let mut timezone = TimeZone::standard("{zone}", standard);
                            timezone.add_daylight(daylight);
                            timezone
                            "#,
                            standard_dtstart.format(FMT_ICS_LOCAL),
                            format_utc_offset(dst_offset),
                            format_utc_offset(utc_offset),
                            dst_dtstart.format(FMT_ICS_LOCAL),
                            format_utc_offset(utc_offset),
                            format_utc_offset(dst_offset),
                        )?;
                    }
                    _ => unimplemented!("more than 2 active rules"),
                }
            }
        }

        writeln!(librs, "}}")?;
    }

    writeln!(librs, "}} }} }}")?;

    Ok(())
}

fn rule_to_rrule(rule: &RuleInfo) -> String {
    match rule.day {
        DaySpec::Ordinal(monthday) => {
            assert!(monthday >= 1);
            format!(
                "FREQ=YEARLY;INTERVAL=1;BYMONTHDAY={};BYMONTH={}",
                monthday, rule.month as u32
            )
        }
        DaySpec::Last(weekday) => {
            format!(
                "FREQ=YEARLY;INTERVAL=1;BYDAY=-1{};BYMONTH={}",
                rrule_weekday(weekday),
                rule.month as u32
            )
        }
        DaySpec::LastOnOrBefore(weekday, monthday) => {
            // See docs below, only the BYMONTHDAY logic is reversed
            let bymonthday = (1..=31)
                .rev()
                .skip_while(|n| *n >= monthday as i32)
                .take(7)
                .map(|n| n.to_string())
                .collect::<Vec<String>>()
                .join(",");

            assert!(monthday >= 1);
            format!(
                "FREQ=YEARLY;INTERVAL=1;BYMONTHDAY={bymonthday};BYDAY={};BYMONTH={};BYSETPOS=-1",
                rrule_weekday(weekday),
                rule.month as u32
            )
        }
        DaySpec::FirstOnOrAfter(weekday, monthday) => {
            // From docs:
            // For example, “Sun>=8” means “the first Sunday on or after the eighth of the month,”
            // This would mean wkd=SU and n = 8
            // In that case generate following RRule
            // "FREQ=YEARLY;INTERVAL=1;BYMONTHDAY=8,9,...,31;BYDAY=SU;BYMONTH=somemonth;BYSETPOS=1"
            // That rrule takes the first day in the given month which is a sunday and on the 8th monthday or later

            let bymonthday = (1..=31)
                .skip_while(|n| *n <= monthday as i32)
                .take(7)
                .map(|n| n.to_string())
                .collect::<Vec<String>>()
                .join(",");

            assert!(monthday >= 1);
            format!(
                "FREQ=YEARLY;INTERVAL=1;BYMONTHDAY={bymonthday};BYDAY={};BYMONTH={};BYSETPOS=1",
                rrule_weekday(weekday),
                rule.month as u32
            )
        }
    }
}

fn rrule_weekday(wkd: Weekday) -> &'static str {
    match wkd {
        Weekday::Sunday => "SU",
        Weekday::Monday => "MO",
        Weekday::Tuesday => "TU",
        Weekday::Wednesday => "WE",
        Weekday::Thursday => "TH",
        Weekday::Friday => "FR",
        Weekday::Saturday => "SA",
    }
}

// Takes a timestamp in seconds and returns a +/-XXXX UTC offset string
//                                               \/\/
//                                 where  hours -`  `- minutes
fn format_utc_offset(timestamp: i64) -> String {
    let hours = timestamp / 60 / 60;
    let minutes = ((timestamp.abs()) / 60) % 60;

    format!("{:+03}{:02}", hours, minutes)
}

// Copied from https://github.com/chronotope/chrono-tz/blob/main/chrono-tz-build/src/lib.rs
// Convert all '/' to '__', all '+' to 'Plus' and '-' to 'Minus', unless
// it's a hyphen, in which case remove it. This is so the names can be used
// as rust identifiers.
fn convert_bad_chars(name: &str) -> String {
    let name = name.replace('/', "__").replace('+', "Plus");
    if let Some(pos) = name.find('-') {
        if name[pos + 1..]
            .chars()
            .next()
            .map(char::is_numeric)
            .unwrap_or(false)
        {
            name.replace('-', "Minus")
        } else {
            name.replace('-', "")
        }
    } else {
        name
    }
}
