import { Fragment, type ReactNode } from 'react';

export type JobData = Record<string, unknown>;

function string(value: unknown) {
  return typeof value === 'string' && value.trim() ? value.trim() : '';
}

function strings(value: unknown) {
  return Array.isArray(value) ? value.map(string).filter(Boolean) : [];
}

function safeUrl(value: unknown) {
  const candidate = string(value);
  try {
    const url = new URL(candidate);
    return url.protocol === 'https:' || url.protocol === 'http:' ? url.href : '';
  } catch { return ''; }
}

function CompanyMark({ company, logo }: { company: string; logo?: unknown }) {
  const logoUrl = safeUrl(logo);
  if (logoUrl) return <img className="company-logo" src={logoUrl} alt={`${company} logo`} />;
  return <span className="company-mark" aria-hidden="true">{company.slice(0, 1).toUpperCase() || '?'}</span>;
}

function Tags({ values }: { values: string[] }) {
  return values.length ? <div className="job-tags">{values.map((value) => <span className="job-tag" key={value}>{value}</span>)}</div> : null;
}

function renderHtmlNode(node: Node, key: string): ReactNode {
  if (node.nodeType === Node.TEXT_NODE) return node.textContent;
  if (node.nodeType !== Node.ELEMENT_NODE) return null;
  const element = node as HTMLElement;
  const children = Array.from(element.childNodes).map((child, index) => renderHtmlNode(child, `${key}-${index}`));
  switch (element.tagName.toLowerCase()) {
    case 'h2': return <h2 key={key}>{children}</h2>;
    case 'h3': return <h3 key={key}>{children}</h3>;
    case 'h4': return <h4 key={key}>{children}</h4>;
    case 'p': return <p key={key}>{children}</p>;
    case 'ul': return <ul key={key}>{children}</ul>;
    case 'ol': return <ol key={key}>{children}</ol>;
    case 'li': return <li key={key}>{children}</li>;
    case 'strong': case 'b': return <strong key={key}>{children}</strong>;
    case 'em': case 'i': return <em key={key}>{children}</em>;
    case 'br': return <br key={key} />;
    case 'a': {
      const href = safeUrl(element.getAttribute('href'));
      return href ? <a key={key} href={href} target="_blank" rel="noreferrer">{children}</a> : <Fragment key={key}>{children}</Fragment>;
    }
    default: return <Fragment key={key}>{children}</Fragment>;
  }
}

function RichDescription({ html, text }: { html?: unknown; text?: unknown }) {
  const markup = string(html);
  if (!markup) return <p>{string(text)}</p>;
  const document = new DOMParser().parseFromString(markup, 'text/html');
  return <>{Array.from(document.body.childNodes).map((node, index) => renderHtmlNode(node, `description-${index}`))}</>;
}

function Header({ job, children }: { job: JobData; children?: ReactNode }) {
  const company = string(job.company) || 'Company not provided';
  const source = safeUrl(job.url);
  return <>
    <div className="job-panel-header"><div className="company-identity"><CompanyMark company={company} logo={job.company_logo} /><div><p className="company-name">{company}</p>{children}</div></div>{source && <a className="source-link" href={source} target="_blank" rel="noreferrer">View source ↗</a>}</div>
    <h2 className="job-title">{string(job.title) || 'Job title not provided'}</h2>
  </>;
}

function Description({ html, text }: { html?: unknown; text?: unknown }) {
  if (!string(html) && !string(text)) return null;
  return <section className="job-description"><h3>Description</h3><div className="rich-description"><RichDescription html={html} text={text} /></div></section>;
}

function JobFrame({ job, children }: { job: JobData; children: ReactNode }) {
  const warnings = strings(job.warnings);
  return <section className="job-card formatted-job-card">{children}{warnings.length > 0 && <p className="warning">{warnings.join(' · ')}</p>}</section>;
}

function WelcomeToTheJungleJob({ job }: { job: JobData }) {
  const locations = strings(job.locations);
  return <JobFrame job={job}>
    <Header job={job}><p className="company-meta">{string(job.company_hq) || locations[0]}</p><p className="company-submeta">{strings(job.industry_tags).join(' · ')}</p></Header>
    <p className="job-meta">{[string(job.job_type), locations.join(' · ')].filter(Boolean).join(' · ')}</p>
    <Description html={job.description_html} text={job.description} />
    {string(job.qualifications) && <section className="job-detail"><h3>Qualifications</h3><p>{string(job.qualifications)}</p></section>}
  </JobFrame>;
}

function WellfoundJob({ job }: { job: JobData }) {
  const remoteLocations = strings(job.remote_locations);
  const remote = job.remote === true ? (remoteLocations.length ? `Remote (${remoteLocations.join(', ')})` : 'Remote') : '';
  const experience = job.years_experience_min != null || job.years_experience_max != null ? `${job.years_experience_min ?? '?'}–${job.years_experience_max ?? '?'} yrs exp` : '';
  return <JobFrame job={job}>
    <Header job={job}><p className="company-meta">{[string(job.company_hq), string(job.company_size) && `${string(job.company_size)} employees`].filter(Boolean).join(' · ')}</p><p className="company-submeta">{[...strings(job.company_tags), ...strings(job.company_type_tags)].join(' · ')}</p></Header>
    {string(job.primary_role) && string(job.primary_role) !== string(job.title) && <p className="job-subtitle">{string(job.primary_role)}</p>}
    <p className="job-meta">{[string(job.job_type), remote, string(job.compensation), experience].filter(Boolean).join(' · ')}</p>
    <Tags values={strings(job.skills)} />
    <Tags values={[job.visa_sponsorship === true ? 'Visa sponsorship' : '', job.allow_relocation === true ? 'Relocation' : ''].filter(Boolean)} />
    <Description text={job.description} />
    {string(job.company_description) && <p className="company-description">{string(job.company_description)}</p>}
  </JobFrame>;
}

function IndeedJob({ job }: { job: JobData }) {
  return <JobFrame job={job}><Header job={job}><p className="company-meta">{string(job.location)}</p></Header><Description text={job.description} /></JobFrame>;
}

function GenericJob({ job }: { job: JobData }) {
  const location = string(job.location) || strings(job.locations).join(' · ');
  return <JobFrame job={job}><Header job={job}><p className="company-meta">{location}</p></Header><p className="job-meta">{string(job.job_type)}</p><Tags values={strings(job.skills)} /><Description html={job.description_html} text={job.description ?? job.qualifications} /></JobFrame>;
}

export function JobPanel({ job }: { job: JobData }) {
  switch (string(job.domain)) {
    case 'welcometothejungle': return <WelcomeToTheJungleJob job={job} />;
    case 'wellfound': return <WellfoundJob job={job} />;
    case 'indeed': return <IndeedJob job={job} />;
    default: return <GenericJob job={job} />;
  }
}
